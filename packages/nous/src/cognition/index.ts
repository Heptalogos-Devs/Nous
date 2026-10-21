import {Output} from 'ai';
import type {Models} from '@nous-wave/platform/models';
import type {SourceDocuments} from '../knowledge/source-documents.js';
import {requireSubject} from '../platform/security.js';
import {parseInstant} from '@nous-wave/platform/time';
import { z } from 'zod';
import {sql} from 'kysely';
import { v7 as uuid } from 'uuid';
import type { CallContext, Ref, TimePoint } from '@nous-wave/contracts/common';
import {ConsumerSchema, canonicalRefKey as refKey } from '@nous-wave/contracts/common';
import {
  ChangeSetSchema, ContextPrepareSchema, ContextViewSchema, FeedbackSchema,
  FormationArtifactSchema, WorkContextBodySchema, WorkContextSchema,
  SeedApplySchema,GrowthResolveSchema,KnowledgeChangeSchema,
  type ContextPrepareInput, type ContextView, type FormationArtifact, type QueryInput,
  type QueryResult, type WorkContext,
} from '@nous-wave/contracts/cognition';
import type {Settings} from '@nous-wave/platform/settings';
import {GrowthSettingsSchema,RetrievalSettingsSchema} from '@nous-wave/contracts/management';
import type { KnowledgeCreate, KnowledgeView,KnowledgePage } from '@nous-wave/contracts/knowledge';
import type { ExperienceItem, MaterialView,SourceResolution } from '@nous-wave/contracts/materials';
import {mayReadScope,requireConsumer } from '../platform/security.js';
import { fail } from '../platform/errors.js';
import { changed, json, now, reference, type Storage } from '../platform/context.js';
import {selectText} from '../platform/text.js';

import type { KnowledgeAdoption } from '../knowledge/index.js';
import type { PreparedSourcePolicies } from '../knowledge/sources.js';

export interface CognitionDependencies extends Storage {
  knowledge: {
    sources:{resolve(ctx:CallContext,refs:Ref[]):Promise<SourceResolution>;prepare(ctx:CallContext,refs:Ref[]):Promise<PreparedSourcePolicies>};
    list(ctx: CallContext, input: { kinds?: KnowledgeCreate['kind'][]; limit?: number }): Promise<KnowledgePage>;
    read(ctx: CallContext, input: { id: string; revision?: number },adoption?:KnowledgeAdoption): Promise<KnowledgeView>;
    create(ctx: CallContext, input: unknown,adoption?:KnowledgeAdoption): Promise<KnowledgeView>;
    revise(ctx: CallContext, input: unknown,adoption?:KnowledgeAdoption): Promise<KnowledgeView>;
  };
  materials: { read(ctx: CallContext, input: { ref: Ref }): Promise<MaterialView> };
  encounters: {
    collect(ctx: CallContext, input: { limit: number; refs?: Ref[];after?:{observed:TimePoint;id:string} }): Promise<ExperienceItem[]>;
    markProcessed(ctx: CallContext, refs: Ref[]): Promise<void>;
  };
  retrieval: {
    query(ctx: CallContext, input: QueryInput): Promise<QueryResult>;
    refresh(ctx: CallContext, views?: KnowledgeView[]): Promise<unknown>;
  };
  models: Models;
  workerContext(ctx:CallContext,role:string):Promise<CallContext>;
  sourceDocuments:SourceDocuments;
  settings:Pick<Settings,'read'>;
  capabilities?(ctx: CallContext): Promise<KnowledgeCreate['kind'][]>;
  currentTime(ctx:CallContext):Promise<TimePoint>;
  operationControl?: {
    pause(ctx: CallContext, id: string): Promise<unknown>;
    inspect(ctx: CallContext, id: string): Promise<unknown>;
  };
}

const WorkBodySchema=WorkContextBodySchema.extend({scope:ConsumerSchema});
const plainText = (view: ContextView) => view.blocks.map(block => block.text).join('\n\n');
const count = (text: string) => Array.from(text).length;
const unique = (refs: Ref[]) => [...new Map(refs.map(ref => [refKey(ref), ref])).values()];
const excerpt = (text: string, limit: number) => Array.from(text).slice(0, Math.max(0, limit)).join('');
const date = (value: string) => parseInstant(value);

export function createCognition(deps: CognitionDependencies) {
  async function formChanges(ctx:CallContext,refs:Ref[],roleId:string,phase:string,data:unknown){requireSubject(ctx);
    const selected=unique(refs);if(!selected.length)fail('invalid_input','认知形成没有已取得的来源');
    const catalog=(await deps.sourceDocuments(ctx,selected)).map((document,index)=>({sourceId:`s${index+1}`,...document}));
    const names=catalog.map(item=>item.sourceId) as [string,...string[]];
    const change=KnowledgeChangeSchema.omit({sourceRefs:true}).extend({sourceIds:z.array(z.enum(names)).min(1)});
    const schema=z.object({groups:z.array(z.object({id:z.string(),reason:z.string(),changes:z.array(change)}))});
    const operationId=`${ctx.trace.operationId}/${phase}`;
    const modelCtx=await deps.workerContext({...ctx,trace:{...ctx.trace,operationId}},roleId);
    const result=await deps.models.text.generate(modelCtx,{roleId,inputRefs:selected,promptParams:{},options:{
      messages:[{role:'user',content:JSON.stringify({data,sourceCatalog:catalog})}],output:Output.object({schema}),
    }});
    const parsed=schema.parse(result.output);
    return {operationId,changes:ChangeSetSchema.parse({groups:parsed.groups.map(group=>({...group,changes:group.changes.map(({sourceIds,...body})=>({...body,sourceRefs:sourceIds.map(id=>catalog.find(item=>item.sourceId===id)!.ref)}))}))})};
  }

  async function readWork(ctx: CallContext, input: { id: string },db=deps.db): Promise<WorkContext> {requireSubject(ctx);
    const row = await db.selectFrom('workContexts').selectAll().where('id', '=', input.id).where('subjectId', '=', ctx.subjectId).executeTakeFirst();
    if (!row) fail('not_found', '工作上下文不存在');
    await deps.authorize(ctx,'contexts.read',reference(deps.installationId,ctx.subjectId,'WorkContext',row.id,row.revision));
    const scope=WorkBodySchema.parse(row.body).scope;if(!mayReadScope(ctx,scope))await deps.authorize(ctx,'data.manage',reference(deps.installationId,ctx.subjectId,'WorkContext',row.id,row.revision));
    return WorkContextSchema.parse({ ...WorkBodySchema.parse(row.body), id: row.id, subjectId: row.subjectId, revision: row.revision, ref: reference(deps.installationId, ctx.subjectId, 'WorkContext', row.id, row.revision), recorded: date(row.recordedAt) });
  }

  async function readView(ctx: CallContext, input: { id: string }): Promise<ContextView> {requireSubject(ctx);
    const row = await deps.db.selectFrom('contextViews').select('body').where('id', '=', input.id).where('subjectId', '=', ctx.subjectId).executeTakeFirst();
    if (!row) fail('not_found', '模型上下文视图不存在');
    const view = ContextViewSchema.parse(row.body);
    await deps.authorize(ctx,'contexts.read',view.ref);
    if(!mayReadScope(ctx,view.consumer))await deps.authorize(ctx,'data.manage',view.ref);
    if (view.effectivePolicy.purpose !== ctx.purpose || ctx.audience.some(audience => !view.effectivePolicy.audience.includes(audience))) fail('forbidden', '上下文视图用途或听众不符');
    await deps.knowledge.sources.resolve(ctx,view.inputPortions.map(portion=>({...portion.ref,selector:portion.selector??portion.ref.selector})));
    return view;
  }

  const contexts = {
    async createWork(ctx: CallContext, input: unknown) {requireSubject(ctx);
      await deps.authorize(ctx, 'contexts.updateWork');
      const body = WorkBodySchema.parse({...input as object,scope:ctx.consumer});
      return deps.transaction(async (db) => {
        const id = uuid();
        await db.insertInto('workContexts').values({ id, subjectId: ctx.subjectId, revision: 1, body: json(body), recordedAt: now().at }).execute();
        await changed(db, ctx, [reference(deps.installationId, ctx.subjectId, 'WorkContext', id, 1)]);
        return readWork(ctx, { id },db);
      });
    },
    async updateWork(ctx: CallContext, input: { id: string; expectedRevision: number; body: unknown }) {requireSubject(ctx);
      await deps.authorize(ctx,'contexts.updateWork',reference(deps.installationId,ctx.subjectId,'WorkContext',input.id,input.expectedRevision));
      await readWork(ctx,input);
      const body = WorkBodySchema.parse({...input.body as object,scope:ctx.consumer});
      return deps.transaction(async (db) => {
        const updated = await db.updateTable('workContexts').set({ body: json(body), revision: input.expectedRevision + 1, recordedAt: now().at }).where('id', '=', input.id).where('subjectId', '=', ctx.subjectId).where('revision', '=', input.expectedRevision).returning('id').executeTakeFirst();
        if (!updated) {
          const current = await readWork(ctx, input,db);
          fail('conflict', '工作上下文已经更新', { currentRevision: current.revision });
        }
        await changed(db, ctx, [reference(deps.installationId, ctx.subjectId, 'WorkContext', input.id, input.expectedRevision + 1)]);
        return readWork(ctx, input,db);
      });
    },
    read: readWork, readView,
    async prepare(ctx: CallContext, input: ContextPrepareInput): Promise<ContextView> {requireSubject(ctx);
      await deps.authorize(ctx, 'contexts.prepare');
      const request = ContextPrepareSchema.parse(input);
      requireConsumer(ctx,request.consumer);
      const settingsSnapshots=(await deps.settings.read(ctx,{namespaces:['platform.roles','platform.modelLimits','growth','retrieval'],scope:{kind:'role',subjectId:ctx.subjectId,roleId:request.role}})).modules;
      const retrievalSettings=RetrievalSettingsSchema.parse(settingsSnapshots.retrieval.values);
      const requiredSources=await deps.knowledge.sources.resolve(ctx,request.requiredSources);
      if (request.consumer.kind === 'subject' && request.consumer.id !== ctx.subjectId) fail('forbidden', '主体消费者身份不符');
      if (ctx.actor.kind === 'worker' && request.consumer.kind !== 'worker') fail('forbidden', '工作者不能取得主体或作者视图');
      const work = request.workContextId ? await readWork(ctx, { id: request.workContextId }) : undefined;
      const reserved = request.budget.outputTokens + request.budget.requiredTokens + request.budget.toolTokens + request.budget.mediaTokens;
      const available = Math.min(request.model.contextLimit - reserved, request.budget.cognitionTokens ?? Number.MAX_SAFE_INTEGER);
      if (available <= 0) fail('budget_exhausted', `本次模型必须输入与输出预留共 ${reserved} token，超出 ${request.model.contextLimit} token 上限`);
      const blocks: ContextView['blocks'] = [];
      const trimmed: ContextView['trimmed'] = [];
      const portions: ContextView['inputPortions'] = [];
      portions.push(...requiredSources.resolvedRefs.map(ref=>({ref,text:'',required:true})));
      let used = 0;
      function add(kind: ContextView['blocks'][number]['kind'], text: string, refs: Ref[], maximum: number, required = false) {
        const original = count(text);
        const allowance = Math.max(0, Math.min(maximum, available - used));
        if (required && original > allowance) fail('budget_exhausted', `必须保留的 ${kind} 区块需要 ${original} token，当前剩余 ${allowance}`);
        const included = excerpt(text, allowance);
        if (included) { const tokens = count(included); blocks.push({ kind, text: included, refs, tokens }); used += tokens; }
        if (count(included) < original) trimmed.push({ ref: refs[0], reason: '上下文预算', originalCodePoints: original, includedCodePoints: count(included) });
        return included;
      }
      if (request.roleTask) add('task', request.roleTask, [], available, true);
      if (request.consumer.kind === 'subject') {
        const enabled = deps.capabilities ? await deps.capabilities(ctx) : undefined;
        const kinds = (['Self', 'Social', 'LanguageConvention', 'Motivation'] as const).filter(kind => !enabled || enabled.includes(kind));
        const facets = kinds.length ? (await deps.knowledge.list(ctx, { kinds, limit: 80 })).items : [];
        for (const facet of facets) {
          const included = add('identity', `${facet.body.content}〔${facet.alias}〕`, [facet.ref], available * 0.15);
          if (included) portions.push({ ref: facet.ref, text: included, required: false });
          if (used >= available * 0.15) break;
        }
      }
      if (request.situation) add('situation', request.situation, [], available, true);
      if (work) add('work', `目标：${work.goal}\n进展：${work.progress}\n待解决：${work.openQuestions.join('；')}\n继续线索：${work.resumeCue}\n${work.temporaryNotes.join('\n')}`, [work.ref], available * 0.35);
      for (const ref of work?.currentRefs ?? []) {
        if (ref.product !== 'nous' || ref.installationId !== deps.installationId || !['Material', 'Memory', 'Episode', 'Journal', 'Schema', 'Concept', 'Self', 'Social', 'LanguageConvention', 'Motivation'].includes(ref.kind)) continue;
        const text = ref.kind === 'Material' ? (await deps.materials.read(ctx, { ref })).text ?? '' : (await deps.knowledge.read(ctx, { id: ref.id, revision: ref.revision })).body.content;
        const included = add('work', text, [ref], available * 0.35);
        if (included) portions.push({ ref, text: included, required: false });
      }
      for (const portion of request.latestMaterials) {
        let text = portion.text, selectedRef = portion.ref;
        const requestedSelector = portion.selector ?? portion.ref.selector;
        if (portion.ref.kind === 'Material' && requestedSelector?.kind === 'image') {
          const material = await deps.materials.read(ctx, { ref: { ...portion.ref, selector: undefined } });
          if (!material.blobRef || !material.mediaType.startsWith('image/')) fail('invalid_input', '图像区域需要已保存的图像原件');
          if (requestedSelector.within) fail('capability_unavailable', '该材料尚未提供复合图像区域的表示');
          const x = requestedSelector.x ?? 0, y = requestedSelector.y ?? 0;
          const width = requestedSelector.width ?? 1 - x, height = requestedSelector.height ?? 1 - y;
          if (width <= 0 || height <= 0 || x + width > 1 || y + height > 1) fail('invalid_input', '图像区域超出归一化边界');
          const selector = { kind: 'image' as const, x, y, width, height };
          const [ref] = (await deps.knowledge.sources.resolve(ctx, [{ ...material.ref, selector }])).resolvedRefs;
          if (!ref) fail('not_found', '本次图像区域不可用');
          // The native host supplies these bytes in its actual messages. This
          // view records their area; it does not turn a label into image text.
          portions.push({ ...portion, ref: { ...ref, selector: undefined }, selector, text: '' });
          continue;
        }
        if (portion.ref.kind === 'Material') {
          const material = await deps.materials.read(ctx, { ref: { ...portion.ref, selector: requestedSelector } });
          if (material.text !== undefined) text = material.text;
          selectedRef = material.ref;
        } else if (['Memory', 'Episode', 'Journal', 'Schema', 'Concept', 'Self', 'Social', 'LanguageConvention', 'Motivation'].includes(portion.ref.kind)) {
          const knowledge = await deps.knowledge.read(ctx, { id: portion.ref.id, revision: portion.ref.revision });
          text = knowledge.body.content; selectedRef = knowledge.ref;
          if(requestedSelector){const selection=selectText(text,requestedSelector);text=selection.text;selectedRef={...selectedRef,selector:selection.selector};}
        }
        const selected = selectedRef.selector ?? requestedSelector;
        if (selected && selected.kind !== 'text' && !portion.required && count(text) > available - used) {
          trimmed.push({ ref: selectedRef, reason: '上下文预算不能完整容纳所选区域', originalCodePoints: count(text), includedCodePoints: 0 });
          continue;
        }
        const included = add('sources', text, [selectedRef], available, portion.required);
        if (included) {
          const start = selected?.kind === 'text' ? selected.start ?? 0 : 0;
          const selector = selected && selected.kind !== 'text' ? selected : { ...selected, kind: 'text' as const, start, end: start + count(included) };
          portions.push({ ...portion, ref: selectedRef, text: included, selector });
        }
      }
      const query = request.query ?? (work?.goal || request.situation);
      if (query && available - used > 0) {
        const memories = await deps.retrieval.query(ctx, { intent: query, workContextId: work?.id, limit:retrievalSettings.limit,exploration:retrievalSettings.exploration,preferences: ['independent_sources', 'diverse'] });
        for (const [index, memory] of memories.items.entries()) {
          const included = add('knowledge', `[${index + 1}] ${memory.excerpt}\n${memory.time.formed.at} · ${memory.formation} · ${memory.stance}`, [memory.ref], available - used);
          if (included) portions.push({ ref: memory.ref, text: included, required: false });
        }
      }
      return deps.transaction(async (db) => {
        const id = uuid();
        const view = ContextViewSchema.parse({
          id, ref: reference(deps.installationId, ctx.subjectId, 'ContextView', id), subjectId: ctx.subjectId,
          consumer: request.consumer, role: request.role, workContextId: work?.id, blocks,
          selectedRefs: unique([...blocks.flatMap(block => block.refs),...portions.map(portion=>({...portion.ref,selector:portion.selector??portion.ref.selector})),...requiredSources.resolvedRefs]), inputPortions: portions, trimmed,
          model: request.model, budget: { availableTokens: available, usedTokens: used, reservedTokens: reserved, estimate: 'unicode_code_points' },
          effectivePolicy: { purpose: ctx.purpose, audience: ctx.audience }, settingsSnapshots,
          observed: request.observed, recorded: now().at,
        });
        await db.insertInto('contextViews').values({ id: view.id, subjectId: ctx.subjectId, body: json(view), recordedAt: view.recorded }).execute();
        return view;
      });
    },
    render: plainText,
  };

  async function feedback(ctx: CallContext, input: unknown) {requireSubject(ctx);
    await deps.authorize(ctx, 'cognition.feedback');
    const request = FeedbackSchema.parse(input);
    for (const ref of request.refs) if (ref.subjectId && ref.subjectId !== ctx.subjectId) fail('forbidden', '使用反馈引用不属于当前主体');
    return deps.transaction(async (db) => {
      await db.insertInto('useEvents').values({ id: uuid(), subjectId: ctx.subjectId, stepKey: request.stepKey, kind: request.kind, refs: json(request.refs), basisRefs: json(request.basisRefs), recordedAt: now().at }).onConflict(conflict => conflict.columns(['subjectId', 'stepKey', 'kind']).doNothing()).execute();
      const saved = await db.selectFrom('useEvents').selectAll().where('subjectId', '=', ctx.subjectId).where('stepKey', '=', request.stepKey).where('kind', '=', request.kind).executeTakeFirstOrThrow();
      if (JSON.stringify(saved.refs) !== JSON.stringify(request.refs) || JSON.stringify(saved.basisRefs) !== JSON.stringify(request.basisRefs)) fail('conflict', '同一步骤已有不同的使用反馈');
      return { id: saved.id, ...request, recorded: date(saved.recordedAt) };
    });
  }

  async function inspect(ctx: CallContext, input: { id: string }): Promise<FormationArtifact> {requireSubject(ctx);
    await deps.authorize(ctx,'growth.inspect',reference(deps.installationId,ctx.subjectId,'FormationArtifact',input.id));
    const row = await deps.db.selectFrom('formationArtifacts').select('body').where('id', '=', input.id).where('subjectId', '=', ctx.subjectId).executeTakeFirst();
    if (!row) fail('not_found', '认知形成成果不存在');
    const artifact = FormationArtifactSchema.parse(row.body);
    await deps.knowledge.sources.resolve(ctx,artifact.sourceRefs);
    return artifact;
  }

  function buildArtifact(ctx:CallContext,changeSet:z.infer<typeof ChangeSetSchema>,experienceRefs:Ref[],sourceRefs:Ref[],enabled:KnowledgeCreate['kind'][]|undefined,purpose:'growth'|'seed',previousArtifactRef?:Ref,modelOperationId?:string){requireSubject(ctx);
    const allowed=new Set([...sourceRefs,...experienceRefs].map(refKey));
    const duplicateIds=new Set(changeSet.groups.filter((group,index,groups)=>groups.findIndex(other=>other.id===group.id)!==index).map(group=>group.id));
    const issues=(group:z.infer<typeof ChangeSetSchema>['groups'][number])=>[...(duplicateIds.has(group.id)?['形成组身份重复']:[]),...group.changes.flatMap(change=>[...(!change.sourceRefs.length?['形成成果需要明确来源']:[]),...(change.sourceRefs.some(source=>!allowed.has(refKey(source)))?['认知形成引用了未取得的来源']:[]),...(enabled&&!enabled.includes(change.kind)?[`主体未启用 ${change.kind}`]:[])])];
    const id=uuid();return FormationArtifactSchema.parse({id,ref:reference(deps.installationId,ctx.subjectId,'FormationArtifact',id),subjectId:ctx.subjectId,purpose,previousArtifactRef,modelOperationId,experienceRefs,sourceRefs,changeSet,groups:changeSet.groups.map(group=>({id:group.id,status:issues(group).length?'invalid':'pending',issues:issues(group),createdRefs:[],revisedRefs:[],conflicts:[]})),recorded:now().at});
  }

  async function saveArtifact(ctx: CallContext, changeSet: z.infer<typeof ChangeSetSchema>, experienceRefs: Ref[], sourceRefs: Ref[], purpose: 'growth' | 'seed' = 'growth', previousArtifactRef?: Ref,modelOperationId?:string) {requireSubject(ctx);
    const artifact=buildArtifact(ctx,changeSet,experienceRefs,sourceRefs,await deps.capabilities?.(ctx),purpose,previousArtifactRef,modelOperationId);
    return deps.transaction(async db=>{
      await db.insertInto('formationArtifacts').values({id:artifact.id,subjectId:ctx.subjectId,body:json(artifact),recordedAt:artifact.recorded}).execute();
      return artifact;
    });
  }

  async function apply(ctx: CallContext, input: { id: string; groupIds?: string[] }) {requireSubject(ctx);
    await deps.authorize(ctx, 'growth.apply');
    const artifact = await inspect(ctx, input);
    if (artifact.purpose === 'seed' && !artifact.previousArtifactRef) fail('invalid_input', '人格种子预览需要通过 seeds.apply 选择并采用');
    const chosen = input.groupIds ?? artifact.changeSet.groups.map(group => group.id);
    if (chosen.some(id => !artifact.groups.some(group => group.id === id))) fail('invalid_input', '所选形成组不存在');
    for (const groupId of chosen) {
      const planned=artifact.changeSet.groups.find(group=>group.id===groupId)!;
      const state=artifact.groups.find(group=>group.id===groupId)!;
      if(['applied','discarded','invalid'].includes(state.status))continue;
      const prepared=await deps.knowledge.sources.prepare(ctx,unique([...artifact.sourceRefs,...artifact.experienceRefs,...planned.changes.flatMap(change=>[...change.sourceRefs,...change.body.conceptRefs,...(change.target?[reference(deps.installationId,ctx.subjectId,change.kind,change.target.id,change.target.expectedRevision)]:[])])]));
      try {
        await deps.transaction(async (db) => {
          const adoption:KnowledgeAdoption={db,external:prepared};
          const row = await db.selectFrom('formationArtifacts').select('body').where('id', '=', artifact.id).where('subjectId', '=', ctx.subjectId).forUpdate().executeTakeFirstOrThrow();
          const current = FormationArtifactSchema.parse(row.body);
          const state = current.groups.find(group => group.id === groupId)!;
          if (state.status === 'applied' || state.status === 'discarded' || state.status === 'invalid') return;
          const group = current.changeSet.groups.find(group => group.id === groupId)!;
          const ordered = [...group.changes].sort((a, b) => (a.target?.id ?? '').localeCompare(b.target?.id ?? ''));
          for (const change of ordered) {
            if (change.target) {
              const existing = await deps.knowledge.read(ctx, change.target,adoption);
              if (existing.revision !== change.target.expectedRevision) fail('conflict', '形成依据的认知修订已经变化', { currentRevision: existing.revision });
            }
          }
          for (const change of ordered) {
            const { target, reason, ...draft } = change;
            const view = target ? await deps.knowledge.revise(ctx, { ...draft, ...target, reason },adoption) : await deps.knowledge.create(ctx, draft,adoption);
            (target ? state.revisedRefs : state.createdRefs).push(view.ref);
          }
          state.status = 'applied';
          state.conflicts = [];
          await db.updateTable('formationArtifacts').set({ body: json(current) }).where('id', '=', current.id).execute();
        });
      } catch (error) {
        if (!(error instanceof Error) || !('code' in error) || error.code !== 'conflict') throw error;
        await deps.transaction(async (db) => {
          const row = await db.selectFrom('formationArtifacts').select('body').where('id', '=', artifact.id).where('subjectId', '=', ctx.subjectId).forUpdate().executeTakeFirstOrThrow();
          const current = FormationArtifactSchema.parse(row.body);
          const state = current.groups.find(group => group.id === groupId)!;
          if (state.status === 'applied') return;
          state.status = 'conflict';
          state.conflicts = current.changeSet.groups.find(group => group.id === groupId)!.changes.flatMap(change => change.target ? [{ ...change.target, currentRevision: 'details' in error ? (error.details as { currentRevision?: number }).currentRevision : undefined }] : []);
          await db.updateTable('formationArtifacts').set({ body: json(current) }).where('id', '=', current.id).execute();
        });
      }
    }
    const current = await inspect(ctx, input);
    const appliedGroups = current.groups.filter(group => group.status === 'applied').map(group => group.id);
    const pendingGroups = current.groups.filter(group => group.status === 'pending' || group.status === 'conflict' || group.status === 'invalid').map(group => group.id);
    if (!pendingGroups.length) await deps.encounters.markProcessed(ctx, current.experienceRefs);
    const createdRefs = current.groups.flatMap(group => group.createdRefs);
    const revisedRefs = current.groups.flatMap(group => group.revisedRefs);
    const views = await Promise.all(unique([...createdRefs, ...revisedRefs]).map(ref => deps.knowledge.read(ctx, { id: ref.id, revision: ref.revision })));
    const index = await deps.retrieval.refresh(ctx, views);
    return { artifact: current, appliedGroups, pendingGroups, createdRefs, revisedRefs, index };
  }

  const growth = {
    inspect, apply,
    async resolve(ctx:CallContext,input:z.input<typeof GrowthResolveSchema>){requireSubject(ctx);
      await deps.authorize(ctx,'growth.resolve');const request=GrowthResolveSchema.parse(input),original=await inspect(ctx,request);
      if(original.purpose!=='growth')fail('invalid_input','人格种子请编辑选择后采用');
      const ids=new Set(request.groupIds),states=original.groups.filter(group=>ids.has(group.id));
      if(states.length!==ids.size||states.some(group=>group.status==='applied'||group.status==='discarded'))fail('invalid_input','只能处理尚未采用或结束的形成组');
      const previous={groups:original.changeSet.groups.filter(group=>ids.has(group.id))};
      let groups=request.groups,invocationId:string|undefined,allowed=original.sourceRefs;
      if(request.action==='edit'&&!groups)fail('invalid_input','编辑形成组需要完整的所选修改');
      if(request.action==='rejudge'){
            const targetIds=[...new Set(previous.groups.flatMap(group=>group.changes.flatMap(change=>change.target?[change.target.id]:[])))];
        const current=await Promise.all(targetIds.map(id=>deps.knowledge.read(ctx,{id})));
        const legal=new Set(original.sourceRefs.map(refKey));
        let relevant=unique(previous.groups.flatMap(group=>group.changes.flatMap(change=>change.sourceRefs))).filter(ref=>legal.has(refKey(ref)));if(!relevant.length)relevant=original.sourceRefs;
        const materials=await Promise.all(relevant.filter(ref=>ref.kind==='Material').map(ref=>deps.materials.read(ctx,{ref})));
        allowed=unique([...allowed,...current.map(view=>view.ref)]);
        const result=await formChanges(ctx,unique([...relevant,...current.map(view=>view.ref)]),'subject_reflection','growth/rejudge',{previous,current,materials,formed:request.formed,groupIds:[...ids]});
        groups=result.changes.groups;invocationId=result.operationId;
      }
      if(request.action!=='discard'&&(groups?.length!==ids.size||groups.some(group=>!ids.has(group.id))||new Set(groups.map(group=>group.id)).size!==ids.size))fail('invalid_input','修改必须且只能覆盖所选形成组');
      const allowedKeys=new Set([...allowed,...original.experienceRefs].map(refKey)),enabled=await deps.capabilities?.(ctx);
      await deps.transaction(async (db)=>{
        const row=await db.selectFrom('formationArtifacts').select('body').where('id','=',original.id).where('subjectId','=',ctx.subjectId).forUpdate().executeTakeFirstOrThrow(),artifact=FormationArtifactSchema.parse(row.body);
        for(const id of ids){if(JSON.stringify(artifact.changeSet.groups.find(group=>group.id===id))!==JSON.stringify(previous.groups.find(group=>group.id===id))||JSON.stringify(artifact.groups.find(group=>group.id===id))!==JSON.stringify(original.groups.find(group=>group.id===id)))fail('conflict','所选形成组已由其他操作处理');}
        artifact.resolutions.push({groupIds:[...ids],previous,action:request.action,reason:request.reason,at:request.formed,modelOperationId:invocationId});artifact.sourceRefs=allowed;
        for(const state of artifact.groups.filter(group=>ids.has(group.id))){
          state.conflicts=[];state.issues=[];
          if(request.action==='discard'){state.status='discarded';continue;}
          const group=groups!.find(group=>group.id===state.id)!;
          group.changes=group.changes.map(change=>({...change,formed:request.formed,formation:['operator_edit','initial_setting'].includes(change.formation)?'reflection':change.formation}));
          for(const change of group.changes){if(!change.sourceRefs.length)state.issues.push('形成成果需要明确来源');if(change.sourceRefs.some(ref=>!allowedKeys.has(refKey(ref))))state.issues.push('认知形成引用了未取得的来源');if(enabled&&!enabled.includes(change.kind))state.issues.push(`主体未启用 ${change.kind}`);}
          artifact.changeSet.groups[artifact.changeSet.groups.findIndex(item=>item.id===state.id)]=group;state.status=state.issues.length?'invalid':'pending';
        }
        await db.updateTable('formationArtifacts').set({body:json(artifact)}).where('id','=',artifact.id).execute();
      });
      return apply(ctx,{id:request.id,groupIds:request.groupIds});
    },
    async collect(ctx: CallContext, input: { refs?: Ref[]; limit?: number } = {}) {requireSubject(ctx);
      const configuration=(await deps.settings.read(ctx,{namespaces:['growth'],scope:{kind:'subject',subjectId:ctx.subjectId}})).modules.growth;
      const configuredLimit=GrowthSettingsSchema.parse(configuration.values).batchSize;
      await deps.authorize(ctx, 'growth.run');
      const limit=Math.min(input.limit??configuredLimit,configuredLimit),selected:ExperienceItem[]=[];
      let after:{observed:TimePoint;id:string}|undefined;
      while(selected.length<limit){
        const page=await deps.encounters.collect(ctx,{refs:input.refs,limit:24,after});if(!page.length)break;
        const predicates=page.map(item=>sql<boolean>`body->'experienceRefs' @> ${JSON.stringify([{id:item.ref.id}])}::jsonb`);
        const assigned=await deps.db.selectFrom('formationArtifacts').select('body').where('subjectId','=',ctx.subjectId).where(eb=>eb.or(predicates)).execute();
        const used=new Set(assigned.flatMap(row=>FormationArtifactSchema.parse(row.body).experienceRefs.map(ref=>ref.id)));
        selected.push(...page.filter(item=>!used.has(item.ref.id)).slice(0,limit-selected.length));
        const last=page.at(-1)!;after={observed:last.observed,id:last.ref.id};if(page.length<24)break;
      }
      return selected;
    },
    async run(ctx: CallContext, input: { experienceRefs?: Ref[]; purpose?: 'reflection' | 'remember' | 'narrative'; formed?: TimePoint; apply?: boolean; maxInputTokens?: number } = {}) {requireSubject(ctx);
      await deps.authorize(ctx, 'growth.run');
      const experiences = await growth.collect(ctx, { refs: input.experienceRefs });
      if (!experiences.length) return { artifact: undefined, appliedGroups: [], pendingGroups: [], createdRefs: [], revisedRefs: [], reason: '没有尚待整理的主体经历' };
      const sources = unique(experiences.flatMap(experience => experience.portions.map(portion => portion.ref)));
      const materialInputs: Array<{ ref: Ref; text: string }> = [];
      for (const ref of sources) {
        if (ref.kind === 'Material') materialInputs.push({ ref, text: (await deps.materials.read(ctx, { ref })).text ?? '' });
        else if (['Memory', 'Episode', 'Journal', 'Schema', 'Concept', 'Self', 'Social', 'LanguageConvention', 'Motivation'].includes(ref.kind)) materialInputs.push({ ref, text: (await deps.knowledge.read(ctx, { id: ref.id, revision: ref.revision })).body.content });
      }
      const existing = (await deps.knowledge.list(ctx, { limit: 80 })).items;
      const formed = input.formed ?? await deps.currentTime(ctx);
      const enabledKinds = await deps.capabilities?.(ctx) ?? ['Memory'];
      const scope = { purpose: input.purpose ?? 'reflection', enabledKinds, experiences: [...experiences], materials: [...materialInputs], currentKnowledge: existing };
      const maximum = Math.min(input.maxInputTokens ?? 12_000, 12_000);
      // Sources remain intact; reduce the batch at source boundaries before invoking the model.
      while (count(JSON.stringify(scope)) > maximum && scope.experiences.length > 1) {
        scope.experiences.pop();
        const keep = new Set(scope.experiences.flatMap(experience => experience.portions.map(portion => refKey(portion.ref))));
        scope.materials = materialInputs.filter(material => keep.has(refKey(material.ref)));
      }
      const retained = new Set(scope.experiences.flatMap(experience => experience.portions.map(portion => refKey(portion.ref))));
      scope.materials = materialInputs.filter(material => retained.has(refKey(material.ref)));
      while (count(JSON.stringify(scope)) > maximum && scope.currentKnowledge.length) scope.currentKnowledge.pop();
      if (count(JSON.stringify(scope)) > maximum) fail('budget_exhausted', '单项实际经历超出成长输入预算，请先取得有来源的摘要或提高允许范围');
      const allowed = unique([...scope.materials.map(material => material.ref), ...scope.currentKnowledge.map(view => view.ref),...scope.experiences.map(item=>item.ref)]);
      const result = await formChanges(ctx,allowed,scope.purpose==='remember'?'memory_formation':'subject_reflection','growth/form',{formed,scope});
      const changes = result.changes;
      for (const group of changes.groups) for (const change of group.changes) {
        change.formed = formed;
        if (change.formation === 'operator_edit' || change.formation === 'initial_setting') change.formation = 'reflection';
      }
      const artifact = await saveArtifact(ctx, changes, scope.experiences.map(experience => experience.ref), allowed,'growth',undefined,result.operationId);
      if (input.apply === false) return { artifact, appliedGroups: [], pendingGroups: artifact.groups.map(group => group.id), createdRefs: [], revisedRefs: [] };
      return apply(ctx, { id: artifact.id });
    },
  };

  const seeds = {
    async preview(ctx: CallContext, input: { refs: Ref[]; instructions?: string }) {requireSubject(ctx);
      await deps.authorize(ctx, 'seeds.preview');
      const materials = await Promise.all(input.refs.map(ref => deps.materials.read(ctx, { ref })));
      const current = (await deps.knowledge.list(ctx, { limit: 200 })).items;
      const seedSources = unique([...input.refs, ...current.map(view => view.ref)]);
      const formed=await deps.currentTime(ctx);
      const result = await formChanges(ctx,seedSources,'self_formation','seeds/interpret',{formed,instructions:input.instructions??'',materials,current});
      const changeSet = result.changes;
      const changes = changeSet.groups.flatMap(group => group.changes).map(change => ({ ...change,formed, formation: 'initial_setting' as const }));
      const artifact = await saveArtifact(ctx, { groups: [{ id: 'seed', reason: '采用所选人格种子', changes }] }, [], seedSources, 'seed',undefined,result.operationId);
      return { artifact, proposedChanges: changes };
    },
    async apply(ctx: CallContext, input: z.input<typeof SeedApplySchema>) {requireSubject(ctx);
      await deps.authorize(ctx, 'seeds.apply');
      const request = SeedApplySchema.parse(input);
      const preview=await inspect(ctx,request);
      if(preview.purpose!=='seed')fail('invalid_input','所选成果不是人格种子预览');
      const proposed=preview.changeSet.groups.flatMap(group=>group.changes);
      if(request.selectedIndexes?.some(index=>index>=proposed.length))fail('invalid_input','所选种子事项不存在');
      const selected=request.changes??proposed.filter((_,index)=>!request.selectedIndexes||request.selectedIndexes.includes(index));
      const next=buildArtifact(ctx,{groups:[{id:'seed',reason:'采用编辑后的人格种子事项',changes:selected.map(change=>({...change,formation:'initial_setting',formed:request.formed}))}]},[],preview.sourceRefs,await deps.capabilities?.(ctx),'seed',preview.ref);
      const artifact=await deps.transaction(async db=>{
        const row=await db.selectFrom('formationArtifacts').select('body').where('id','=',request.id).where('subjectId','=',ctx.subjectId).forUpdate().executeTakeFirstOrThrow();
        const current=FormationArtifactSchema.parse(row.body);
        if(current.appliedArtifactRef){
          const applied=await db.selectFrom('formationArtifacts').select('body').where('id','=',current.appliedArtifactRef.id).where('subjectId','=',ctx.subjectId).executeTakeFirstOrThrow();
          return FormationArtifactSchema.parse(applied.body);
        }
        if(JSON.stringify(current.changeSet)!==JSON.stringify(preview.changeSet))fail('conflict','种子预览已经修改');
        await db.insertInto('formationArtifacts').values({id:next.id,subjectId:ctx.subjectId,body:json(next),recordedAt:next.recorded}).execute();
        await db.updateTable('formationArtifacts').set({body:json({...current,appliedArtifactRef:next.ref})}).where('id','=',current.id).execute();
        return next;
      });
      return apply(ctx, { id: artifact.id });
    },
  };

  const maintenance = {
    async run(ctx: CallContext, input: { kind: 'index' | 'growth' } = { kind: 'growth' }) {requireSubject(ctx);
      await deps.authorize(ctx, 'maintenance.run');
      return input.kind === 'index' ? deps.retrieval.refresh(ctx) : growth.run(ctx);
    },
    async inspect(ctx: CallContext, input: { id: string }) {requireSubject(ctx); await deps.authorize(ctx, 'maintenance.inspect'); if (!deps.operationControl) fail('capability_unavailable', '维护执行控制尚未配置'); return deps.operationControl.inspect(ctx, input.id); },
    async pause(ctx: CallContext, input: { id: string }) {requireSubject(ctx);
      await deps.authorize(ctx, 'maintenance.pause');
      if (!deps.operationControl) fail('capability_unavailable', '维护执行控制尚未配置');
      return deps.operationControl.pause(ctx, input.id);
    },
  };
  return { contexts, growth, seeds, maintenance, feedback };
}
