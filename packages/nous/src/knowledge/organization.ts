import { z } from 'zod';
import { v7 as uuid } from 'uuid';
import { Temporal } from '@js-temporal/polyfill';
import { canonicalRefKey, type CallContext, type Ref, type TimePoint, type TimeRange } from '@nous-wave/contracts/common';
import {
  ConceptMergeSchema, ConceptSplitSchema, EpisodeOrganizeSchema, FactPredicateSchema,
  JournalWriteSchema, MotivationEvaluateSchema, MotivationEvaluationSchema,
  type KnowledgeBody, type KnowledgeCreate, type KnowledgeView,type KnowledgePage,KnowledgeListSchema,
} from '@nous-wave/contracts/knowledge';
import type { ExperienceItem, MaterialView,ExperiencePage,ExperienceListSchema } from '@nous-wave/contracts/materials';
import { authorize } from '../platform/security.js';
import { fail } from '../platform/errors.js';
import { json, now, reference, type Storage } from '../platform/context.js';

interface KnowledgeOperations {
  read(ctx: CallContext, input: { id: string; revision?: number; knowledgeAt?: TimePoint }): Promise<KnowledgeView>;
  list(ctx: CallContext, input?: z.input<typeof KnowledgeListSchema>): Promise<KnowledgePage>;
  create(ctx: CallContext, input: unknown): Promise<KnowledgeView>;
  revise(ctx: CallContext, input: unknown): Promise<KnowledgeView>;
  setStatus(ctx: CallContext, input: { id: string; expectedRevision: number; status: 'active' | 'suppressed' | 'retired' }): Promise<KnowledgeView>;
  relate(ctx: CallContext, input: { fromId: string; toId: string; relation: string; weight?: number; evidenceRefs: Ref[];formed:TimePoint }): Promise<unknown>;
}
export interface OrganizationDependencies extends Storage {
  knowledge: KnowledgeOperations;
  materials: { read(ctx: CallContext, input: { ref: Ref }): Promise<MaterialView> };
  encounters: { inputs(ctx: CallContext, ref: Ref): Promise<Ref[]>; experiences(ctx: CallContext,input?:z.input<typeof ExperienceListSchema>): Promise<ExperiencePage> };
  model?: { generate(ctx: CallContext, input: { role: string; prompt: string; schema?: z.ZodType; inputRefs?: Ref[] }): Promise<{ output: unknown }> };
  refresh(ctx: CallContext, views: KnowledgeView[]): Promise<unknown>;
}

type Truth = boolean | undefined;
const unique = (refs: Ref[]) => [...new Map(refs.map(ref => [canonicalRefKey(ref), ref])).values()];
const compare = (a: string, b: string) => Temporal.Instant.compare(Temporal.Instant.from(a), Temporal.Instant.from(b));
const before = (a: TimePoint, b: TimePoint) => a.clockId === b.clockId && compare(a.at, b.at) < 0;
function rangeValid(range: TimeRange) {
  if (range.start && range.end && (range.start.clockId !== range.end.clockId || compare(range.start.at, range.end.at) > 0)) fail('invalid_input', '时间范围的时钟或顺序无效');
}
function inRange(point: TimePoint, range: TimeRange) {
  return (!range.start || point.clockId === range.start.clockId && compare(point.at, range.start.at) >= 0) && (!range.end || point.clockId === range.end.clockId && compare(point.at, range.end.at) < 0);
}

export function evaluatePredicate(input: unknown, predicate: z.infer<typeof FactPredicateSchema>): Truth {
  let value: unknown = input;
  for (const key of predicate.path) {
    if (['__proto__', 'prototype', 'constructor'].includes(key)) fail('invalid_input', '事实属性路径无效');
    if (value === null || typeof value !== 'object' || !Object.hasOwn(value, key)) return predicate.operator === 'exists' ? false : undefined;
    value = (value as Record<string, unknown>)[key];
  }
  if (predicate.operator === 'exists') return value !== undefined && value !== null;
  if (predicate.operator === 'eq') return value === predicate.value;
  if (predicate.operator === 'neq') return value !== predicate.value;
  if (predicate.operator === 'contains') return typeof value === 'string' && typeof predicate.value === 'string' ? value.includes(predicate.value) : Array.isArray(value) ? value.includes(predicate.value) : undefined;
  if (typeof value !== 'number' || typeof predicate.value !== 'number') return undefined;
  return ({ gt: value > predicate.value, gte: value >= predicate.value, lt: value < predicate.value, lte: value <= predicate.value } as Record<string, boolean>)[predicate.operator];
}

export function criterionVerdict(mode: 'achieve' | 'terminate' | 'maintain' | 'avoid', values: Truth[], expired: boolean, covered: boolean): 'satisfied' | 'violated' | 'unknown' {
  if (mode === 'achieve') return values.includes(true) ? 'satisfied' : expired && covered && !values.includes(undefined) ? 'violated' : 'unknown';
  if (mode === 'terminate') return values.includes(false) ? 'satisfied' : expired && covered && values.length > 0 && !values.includes(undefined) ? 'violated' : 'unknown';
  if (mode === 'maintain') return values.includes(false) ? 'violated' : expired && covered && values.length > 0 && !values.includes(undefined) ? 'satisfied' : 'unknown';
  return values.includes(true) ? 'violated' : expired && covered && !values.includes(undefined) ? 'satisfied' : 'unknown';
}

export function createOrganization(deps: OrganizationDependencies) {
  const draft = (old: KnowledgeView, body: KnowledgeBody, formed: TimePoint, sources = old.sourceRefs): KnowledgeCreate => ({ kind: body.kind, body, sourceRefs: sources, formed, valid: old.valid, formation: 'reflection', stance: old.stance, confidence: old.confidence, policy: old.policy });
  async function readTarget(ctx: CallContext, target: { id: string; expectedRevision: number }, kind?: KnowledgeCreate['kind']) {
    const view = await deps.knowledge.read(ctx, target);
    if (view.revision !== target.expectedRevision) fail('conflict', '组织依据的认识已修订', { currentRevision: view.revision });
    if (kind && view.kind !== kind) fail('invalid_input', `目标需要是 ${kind}`);
    return view;
  }
  async function sourceDocs(ctx: CallContext, refs: Ref[]) {
    const result: Array<{ ref: Ref; text: string; at?: TimePoint }> = [];
    for (const ref of unique(refs)) {
      if (ref.kind === 'Material') {
        const material = await deps.materials.read(ctx, { ref });
        result.push({ ref, text: material.text ?? '', at: material.occurred?.start });
      } else if (ref.kind === 'ExperienceItem') {
        const experience = (await deps.encounters.experiences(ctx,{refs:[ref],limit:1})).items[0];
        if (!experience) fail('not_found', '主体经历不存在');
        const inputs = await sourceDocs(ctx, experience.portions.map(portion => portion.ref));
        result.push({ ref, text: inputs.map(input => input.text).join('\n'), at: experience.observed });
      } else {
        const view = await deps.knowledge.read(ctx, { id: ref.id, revision: ref.revision });
        result.push({ ref: view.ref, text: view.body.content, at: view.valid?.start ?? view.formed });
      }
    }
    return result;
  }

  const episodes = {
    async organize(ctx: CallContext, input: z.input<typeof EpisodeOrganizeSchema>) {
      authorize(ctx, 'episodes.organize');
      const request = EpisodeOrganizeSchema.parse(input);
      const allowed = new Set(request.experienceRefs.map(canonicalRefKey));
      const experiences = new Map((await deps.encounters.experiences(ctx,{refs:request.experienceRefs,limit:200})).items.map(experience => [experience.ref.id, experience]));
      const proposedParents = new Map(request.episodes.flatMap(plan => plan.target ? [[plan.target.id, plan.parentRef] as const] : []));
      for (const ref of request.experienceRefs) {
        if (ref.kind !== 'ExperienceItem') fail('invalid_input', '经历分段成员必须指向实际主体经历');
        await deps.encounters.inputs(ctx, ref);
      }
      for (const plan of request.episodes) {
        rangeValid(plan.range);
        if (plan.memberRefs.some(ref => !allowed.has(canonicalRefKey(ref)))) fail('invalid_input', '分段引用了本次未选择的经历');
        if (plan.memberRefs.some(ref => !experiences.has(ref.id) || !inRange(experiences.get(ref.id)!.observed, plan.range))) fail('invalid_input', '经历成员的实际观察时间超出本次片段范围');
        const seen = new Set(plan.target ? [plan.target.id] : []);
        let parent = plan.parentRef;
        for (let depth = 0; parent; depth++) {
          if (seen.has(parent.id) || depth >= 64) fail('invalid_input', '经历父子关系不能形成环');
          seen.add(parent.id);
          if (proposedParents.has(parent.id)) { parent = proposedParents.get(parent.id); continue; }
          const view = await deps.knowledge.read(ctx, { id: parent.id, revision: parent.revision });
          if (view.body.kind !== 'Episode' || view.body.track !== request.track) fail('invalid_input', '父经历必须属于同一组织轨道');
          parent = view.body.parentRef;
        }
      }
      const items = await deps.transaction(async () => {
        const changed: KnowledgeView[] = [];
        for (const plan of request.episodes) {
          const old = plan.target ? await readTarget(ctx, plan.target, 'Episode') : undefined;
          if (old?.body.kind === 'Episode' && old.body.track !== request.track) fail('invalid_input', '经历身份不能改到另一条组织轨道');
          const body: KnowledgeBody = { kind: 'Episode', title: plan.title, content: plan.summary, track: request.track, range: plan.range, members: plan.memberRefs, participants: plan.participantRefs, placeRef: plan.placeRef, parentRef: plan.parentRef, entityRefs: plan.participantRefs, conceptRefs: [], tags: [] };
          const create: KnowledgeCreate = { kind: 'Episode', body, sourceRefs: plan.memberRefs, formed: request.formed, formation: 'reflection', stance: 'accepted', confidence: 0.8 };
          const view = old ? await deps.knowledge.revise(ctx, { ...create, ...plan.target, reason: request.reason, policy: old.policy }) : await deps.knowledge.create(ctx, create);
          for (const ref of plan.memberRefs) await deps.db.insertInto('episodeMembers').values({ episodeId: view.ref.id, revision: view.revision, experienceId: ref.id, subjectId: ctx.subjectId, track: request.track }).onConflict(conflict => conflict.columns(['episodeId', 'revision', 'experienceId']).doNothing()).execute();
          changed.push(view);
        }
        for (const target of request.replaced) {
          const old = await readTarget(ctx, target, 'Episode');
          if (changed.some(view => view.ref.id === old.ref.id)) continue;
          for (const replacement of changed) await deps.knowledge.relate(ctx, { fromId: old.ref.id, toId: replacement.ref.id, relation: 'reorganized_as',formed:request.formed, evidenceRefs: request.experienceRefs });
          await deps.knowledge.setStatus(ctx, { ...target, status: 'retired' });
        }
        return changed;
      });
      return { items, replacedRefs: request.replaced.map(target => reference(deps.installationId, ctx.subjectId, 'Episode', target.id, target.expectedRevision)), index: await deps.refresh(ctx, items) };
    },
    async forExperience(ctx: CallContext, ref: Ref) {
      authorize(ctx, 'knowledge.read');
      await deps.encounters.inputs(ctx, ref);
      const rows = await deps.db.selectFrom('episodeMembers').select(['episodeId', 'revision']).where('subjectId', '=', ctx.subjectId).where('experienceId', '=', ref.id).execute();
      const items: KnowledgeView[] = [];
      for (const row of rows) { const view = await deps.knowledge.read(ctx, { id: row.episodeId }); if (view.status === 'active' && view.revision === row.revision) items.push(view); }
      return items;
    },
  };

  const journals = {
    async write(ctx: CallContext, input: z.input<typeof JournalWriteSchema>) {
      authorize(ctx, 'journals.write');
      const request = JournalWriteSchema.parse(input);
      rangeValid(request.range);
      const sources = await sourceDocs(ctx, request.experienceRefs);
      let formationSources = request.experienceRefs;
      let authored: { content?: string; title?: string; mood?: string; outlook?: string } = { content: request.content, title: request.title, mood: request.mood, outlook: request.outlook };
      if (!authored.content) {
        if (!deps.model) fail('capability_unavailable', '日记形成模型尚未配置');
        const schema = z.object({ content: z.string().min(1), title: z.string().optional(), mood: z.string().optional(), outlook: z.string().optional() });
        const known = (await deps.knowledge.list(ctx, { knowledgeAt: request.knowledgeAt, limit: 24 })).items;
        formationSources = unique([...request.experienceRefs, ...known.map(view => view.ref)]);
        authored = schema.parse((await deps.model.generate(ctx, { role: 'growth', schema, inputRefs: formationSources, prompt: `以主体当时视角组织真实经历为日记，只使用给出的材料和该知识截点已有的认识。后来写作保留当时视角，明确未知。\n${JSON.stringify({ range: request.range, knowledgeAt: request.knowledgeAt, sources, known })}` })).output);
      }
      const body: KnowledgeBody = { kind: 'Journal', content: authored.content!, title: authored.title, mood: authored.mood, outlook: authored.outlook, range: request.range, experienceRefs: request.experienceRefs, perspectiveAt: request.knowledgeAt, entityRefs: [], conceptRefs: [], tags: [] };
      const view = await deps.transaction(async () => {
        const create: KnowledgeCreate = { kind: 'Journal', body, formed: request.formed, sourceRefs: formationSources, formation: 'reflection', stance: 'accepted', confidence: 0.8 };
        if (!request.target) return deps.knowledge.create(ctx, create);
        await readTarget(ctx, request.target, 'Journal');
        return deps.knowledge.revise(ctx, { ...create, ...request.target, reason: request.reason });
      });
      return { ...view, index: await deps.refresh(ctx, [view]) };
    },
  };

  const concepts = {
    async merge(ctx: CallContext, input: z.input<typeof ConceptMergeSchema>) {
      authorize(ctx, 'concepts.merge');
      const request = ConceptMergeSchema.parse(input);
      if (request.merged.some(target => target.id === request.kept.id) || new Set(request.merged.map(target => target.id)).size !== request.merged.length) fail('invalid_input', '合并概念需要不同身份');
      const result = await deps.transaction(async () => {
        const kept = await readTarget(ctx, request.kept, 'Concept');
        const others: KnowledgeView[] = [];
        for (const target of [...request.merged].sort((a, b) => a.id.localeCompare(b.id))) others.push(await readTarget(ctx, target, 'Concept'));
        if (kept.body.kind !== 'Concept') fail('invalid_input', '保留目标需要是概念');
        const keptBody = kept.body;
        const removed = new Set(others.map(view => view.ref.id));
        const names = others.flatMap(view => view.body.kind === 'Concept' ? [view.body.name, ...view.body.aliases] : []);
        const body: KnowledgeBody = { ...keptBody, content: request.content ?? keptBody.content, aliases: [...new Set([...keptBody.aliases, ...names])].filter(name => name !== keptBody.name) };
        let current = await deps.knowledge.revise(ctx, { ...draft(kept, body, request.formed, unique([...kept.sourceRefs, ...others.map(view => view.ref)])), ...request.kept, reason: request.reason });
        const revised: KnowledgeView[] = [current];
        let cursor:string|undefined;
        do {
        const page=await deps.knowledge.list(ctx,{conceptIds:[...removed],limit:100,cursor});cursor=page.nextCursor;
        for (const member of page.items) {
          if (removed.has(member.ref.id)) continue;
          if (!member.body.conceptRefs.some(ref => removed.has(ref.id))) continue;
          const refs = unique(member.body.conceptRefs.map(ref => removed.has(ref.id) ? { ...current.ref, revision: undefined } : ref));
          const updated = await deps.knowledge.revise(ctx, { ...draft(member, { ...member.body, conceptRefs: refs }, request.formed), id: member.ref.id, expectedRevision: member.revision, reason: request.reason });
          revised.push(updated);
          if (member.ref.id === current.ref.id) current = updated;
        }
        } while(cursor);
        for (const old of others) {
          await deps.knowledge.relate(ctx, { fromId: old.ref.id, toId: current.ref.id, relation: 'merged_into',formed:request.formed, evidenceRefs: unique([...old.sourceRefs, ...kept.sourceRefs]) });
          await deps.knowledge.setStatus(ctx, { id: old.ref.id, expectedRevision: old.revision, status: 'retired' });
        }
        return { kept: current, revised, mergedRefs: others.map(view => view.ref) };
      });
      return { ...result, index: await deps.refresh(ctx, result.revised) };
    },
    async split(ctx: CallContext, input: z.input<typeof ConceptSplitSchema>) {
      authorize(ctx, 'concepts.split');
      const request = ConceptSplitSchema.parse(input);
      const result = await deps.transaction(async () => {
        const source = await readTarget(ctx, request.source, 'Concept');
        if (source.body.kind !== 'Concept') fail('invalid_input', '拆分目标需要是概念');
        const branches: KnowledgeView[] = [];
        for (const branch of request.branches) {
          const view = await deps.knowledge.create(ctx, { kind: 'Concept', body: { kind: 'Concept', content: branch.definition, name: branch.name, aliases: branch.aliases, language: source.body.language }, sourceRefs: [source.ref], formed: request.formed, formation: 'reflection', stance: source.stance, confidence: source.confidence });
          await deps.knowledge.relate(ctx, { fromId: source.ref.id, toId: view.ref.id, relation: 'split_into',formed:request.formed, evidenceRefs: source.sourceRefs });
          branches.push(view);
        }
        const assignments = new Map<string, { ref: Ref; concepts: Ref[] }>();
        for (const [index, branch] of request.branches.entries()) for (const ref of branch.memberRefs) {
          if (ref.id === source.ref.id) fail('invalid_input', '拆分成员需要是原概念之外的认识对象');
          const entry = assignments.get(ref.id) ?? { ref, concepts: [] };
          entry.concepts.push({ ...branches[index]!.ref, revision: undefined });
          assignments.set(ref.id, entry);
        }
        const revised: KnowledgeView[] = [];
        for (const assignment of assignments.values()) {
          const member = await deps.knowledge.read(ctx, { id: assignment.ref.id, revision: assignment.ref.revision });
          if (!member.body.conceptRefs.some(ref => ref.id === source.ref.id)) fail('invalid_input', '拆分成员未引用原概念');
          revised.push(await deps.knowledge.revise(ctx, { ...draft(member, { ...member.body, conceptRefs: unique([...member.body.conceptRefs.filter(ref => ref.id !== source.ref.id), ...assignment.concepts]) }, request.formed), id: member.ref.id, expectedRevision: member.revision, reason: request.reason }));
        }
        if (request.remainingDefinition) revised.push(await deps.knowledge.revise(ctx, { ...draft(source, { ...source.body, content: request.remainingDefinition }, request.formed), ...request.source, reason: request.reason }));
        return { sourceRef: source.ref, branches, revised };
      });
      return { ...result, index: await deps.refresh(ctx, [...result.branches, ...result.revised]) };
    },
  };

  const motivation = {
    async evaluate(ctx: CallContext, input: z.input<typeof MotivationEvaluateSchema>) {
      authorize(ctx, 'motivation.evaluate');
      const request = MotivationEvaluateSchema.parse(input);
      const object = await deps.knowledge.read(ctx, request);
      if (object.body.kind !== 'Motivation' || !object.body.criterion) fail('invalid_input', '所选动机没有可评价条件');
      const criterion = object.body.criterion;
      rangeValid(criterion.range);
      rangeValid(request.coverage.range);
      if (request.coverage.ratio > 0 && !request.coverage.sourceRefs.length) fail('invalid_input', '观察覆盖需要实际来源');
      await sourceDocs(ctx, request.coverage.sourceRefs);
      const observations = await sourceDocs(ctx, request.observationRefs);
      const applicable = (!criterion.range.start || criterion.range.start.clockId === request.evaluatedAt.clockId && !before(request.evaluatedAt, criterion.range.start)) && (!criterion.range.end || criterion.range.end.clockId === request.evaluatedAt.clockId);
      const expired = Boolean(criterion.range.end && criterion.range.end.clockId === request.evaluatedAt.clockId && compare(request.evaluatedAt.at, criterion.range.end.at) >= 0);
      const coverageCoversRange = (!criterion.range.start || request.coverage.range.start && request.coverage.range.start.clockId === criterion.range.start.clockId && compare(request.coverage.range.start.at, criterion.range.start.at) <= 0) && (!criterion.range.end || request.coverage.range.end && request.coverage.range.end.clockId === criterion.range.end.clockId && compare(request.coverage.range.end.at, criterion.range.end.at) >= 0);
      const covered = Boolean(coverageCoversRange && request.coverage.ratio >= criterion.requiredCoverage && !request.coverage.missing.length);
      let values: Truth[] = observations.map(observation => {
        if (!observation.at || !inRange(observation.at, criterion.range)) return undefined;
        if (!criterion.predicate) return undefined;
        try { return evaluatePredicate(JSON.parse(observation.text), criterion.predicate); } catch (error) { if (error instanceof SyntaxError) return undefined; throw error; }
      });
      if (applicable && !criterion.predicate && observations.length) {
        if (!deps.model) fail('capability_unavailable', '语义条件评价模型尚未配置');
        const schema = z.object({ matches: z.array(z.object({ sourceIndex: z.number().int().nonnegative(), truth: z.enum(['true', 'false', 'unknown']) })) });
        const output = schema.parse((await deps.model.generate(ctx, { role: 'growth', schema, inputRefs: unique([object.ref, ...request.observationRefs, ...request.coverage.sourceRefs]), prompt: `逐项判断实际材料是否符合给出的条件，保持unknown，不从缺少消息推断未发生。只评价所给观察序号。\n${JSON.stringify({ condition: criterion.condition, range: criterion.range, observations })}` })).output);
        values = observations.map((observation, index) => !observation.at || !inRange(observation.at, criterion.range) ? undefined : ({ true: true, false: false, unknown: undefined })[output.matches.find(match => match.sourceIndex === index)?.truth ?? 'unknown']);
      }
      const verdict = applicable ? criterionVerdict(criterion.mode, values, expired, covered) : 'unknown';
      return deps.transaction(async () => {
        const id = uuid();
        const evaluation = MotivationEvaluationSchema.parse({ id, ref: reference(deps.installationId, ctx.subjectId, 'MotivationEvaluation', id), criterionRef: object.ref, criterionRevision: object.revision, verdict, applicable, observationCoverage: request.coverage, basisRefs: unique([...request.observationRefs, ...request.coverage.sourceRefs]), evaluatedAt: request.evaluatedAt, recorded: now() });
        await deps.db.insertInto('motivationEvaluations').values({ id, subjectId: ctx.subjectId, objectId: object.ref.id, criterionRevision: object.revision, body: json(evaluation), recordedAt: evaluation.recorded.at }).execute();
        return evaluation;
      });
    },
    async history(ctx: CallContext, id: string) {
      authorize(ctx, 'knowledge.read');
      const view = await deps.knowledge.read(ctx, { id });
      const rows = await deps.db.selectFrom('motivationEvaluations').select('body').where('subjectId', '=', ctx.subjectId).where('objectId', '=', view.ref.id).orderBy('recordedAt', 'desc').execute();
      const items = rows.map(row => MotivationEvaluationSchema.parse(row.body));
      for (const item of items) await sourceDocs(ctx, item.basisRefs);
      return items;
    },
  };
  const persona = { async read(ctx: CallContext, input: { format?: 'json' | 'markdown' } = {}) { const facets:KnowledgeView[]=[];let cursor:string|undefined;do{const page=await deps.knowledge.list(ctx,{kinds:['Self'],limit:100,cursor});facets.push(...page.items);cursor=page.nextCursor;}while(cursor); return input.format === 'markdown' ? facets.map(facet => `## ${facet.body.title ?? (facet.body.kind === 'Self' ? facet.body.aspect : 'Self')}\n\n${facet.body.content}`).join('\n\n') : { facets }; } };
  return { episodes, journals, concepts, motivation, persona };
}
