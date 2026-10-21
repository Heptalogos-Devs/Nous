// Resource/revision workflows ported from Nous PromptRegistry and MaiBot PromptManager.
// Copyright 2026 Aravine Zhu and MaiBot contributors; SPDX-License-Identifier: AGPL-3.0-only
import {z} from 'zod';
import {v7 as uuidv7} from 'uuid';
import {LRUCache} from 'lru-cache';
import type {Kysely} from 'kysely';
import type {CallContext,Problem} from '@nous-wave/contracts/common';
import {PromptIdSchema,PromptLocaleSchema,PromptScopeSchema,PromptBindingSchema,PromptSaveSchema,PromptActivateSchema,PromptResetSchema,PromptReadSchema,PromptSnapshotRequestSchema,PromptPreviewSchema,PromptSnapshotRefSchema} from '@nous-wave/contracts/prompts';
import type {PromptScope,PromptBinding,PromptSnapshot,PromptSnapshotRef,PromptSave,PromptActivate,PromptReset,PromptChangeResult,PromptRead,PromptReadResult} from '@nous-wave/contracts/prompts';
import {sql} from '../storage/index.js';
import {problem} from '../errors.js';
import {parse,immutable,digest as jsonDigest,assertJson,isObject} from '../settings/catalog.js';
import type {Settings} from '../settings/index.js';
import {PromptRegistry,checkText} from './registry.js';
import {closure,renderSources,validateExamples,ENGINE_VERSION,optionsDigest} from './engine.js';
import {PromptsSettingsSchema,promptsDefaults} from './config.js';
import type {PromptDatabase} from './migration.js';
import type {PromptAuthorize,PromptDescriptor,PromptDescription,PromptLimits,PromptManifest,PromptSource} from './types.js';
import {createEditor} from './editor.js';
export * from '@nous-wave/contracts/prompts';
export * from './types.js';
export {promptsDefinition,promptsDefaults,PromptsSettingsSchema} from './config.js';
export {promptsMigration,type PromptDatabase} from './migration.js';
export {PromptRegistry} from './registry.js';
export {createPromptDataLifecycle,promptDataMigration} from './data.js';
export interface PromptsOptions {
  installationId:string;db:Kysely<PromptDatabase>;programRoot:string;overrideRoot:string;
  catalog:readonly PromptDescriptor[];settings:Settings;authorize:PromptAuthorize;processBootId?:string;
  onError?:(problem:Problem)=>void;
  watchContext?:()=>Promise<CallContext>;
}
const json=<T>(value:T)=>sql<T>`${JSON.stringify(value)}::jsonb`;
const scopeId=(scope:PromptScope)=>scope.kind==='installation'?scope.installationId:scope.subjectId;
const scopeKey=(scope:PromptScope)=>jsonDigest(scope);
function num(value:string|undefined|null){const result=value==null?0:Number(value);if(!Number.isSafeInteger(result)||result<0)throw problem('prompts.invalid_revision','failed','Prompt修订无效');return result;}
export function createPrompts(options:PromptsOptions){
  const {db,authorize,installationId}=options;
  const processBootId=options.processBootId??uuidv7();let closed=false;let closing:Promise<void>|undefined;
  const registry=new PromptRegistry(options.programRoot,options.overrideRoot);
  const catalog=new Map<string,PromptDescriptor>();const descriptions=new Map<string,PromptDescription>();
  for(const descriptor of options.catalog){
    parse(PromptIdSchema,descriptor.id);parse(PromptLocaleSchema,descriptor.defaultLocale);
    if(catalog.has(descriptor.id)||!descriptor.programFiles[descriptor.defaultLocale]||!descriptor.locales.includes(descriptor.defaultLocale))throw problem('prompts.invalid_descriptor','validation','Prompt目录定义重复或缺少默认语言资源');
    for(const locale of descriptor.locales)parse(PromptLocaleSchema,locale);
    for(const id of descriptor.allowedPartials)parse(PromptIdSchema,id);
    const paramsSchema=z.toJSONSchema(descriptor.paramsSchema);assertJson(paramsSchema);
    if(!isObject(paramsSchema))throw problem('prompts.invalid_descriptor','validation','Prompt参数合同必须为JSON Schema对象');
    const description={id:descriptor.id,owner:descriptor.owner,labelKey:descriptor.labelKey,descriptionKey:descriptor.descriptionKey,visibility:descriptor.visibility,locales:[...descriptor.locales],defaultLocale:descriptor.defaultLocale,paramsSchema,paramsSchemaRevision:descriptor.paramsSchemaRevision,allowedPartials:[...descriptor.allowedPartials]};
    descriptions.set(descriptor.id,immutable({...description,catalogRevision:jsonDigest({...description,programFiles:descriptor.programFiles})}));
    assertJson(descriptor.examples);
    catalog.set(descriptor.id,{...descriptor,examples:immutable(structuredClone(descriptor.examples)),programFiles:immutable({...descriptor.programFiles}),allowedPartials:Object.freeze([...descriptor.allowedPartials])});
  }
  const listeners=new Set<(change:{promptId:string;locale:string;scope:PromptScope;revision:number})=>void>();
  type ConsumerEntry={consumer:ReturnType<Settings['registerConsumer']>;reading:number;evicted:boolean};
  const consumers=new LRUCache<string,ConsumerEntry>({max:128,dispose:entry=>{entry.evicted=true;if(!entry.reading)entry.consumer.close();}});
  let installationConsumer:ReturnType<Settings['registerConsumer']>|undefined;
  function descriptor(id:string){const result=catalog.get(id);if(!result)throw problem('prompts.not_found','not_found','Prompt未注册');return result;}
  function scope(input:PromptScope){if(closed)throw problem('prompts.closed','unavailable','Prompt实例已关闭');const parsed=parse(PromptScopeSchema,input);if(parsed.kind==='installation'&&parsed.installationId!==installationId)throw problem('prompts.installation_mismatch','validation','Prompt安装标识不匹配');return parsed;}
  async function config(target:PromptScope){
    const registration={namespace:'platform.prompts',consumerId:'prompts.registry',processBootId,scope:target};
    const key=scopeKey(target);let entry:ConsumerEntry;
    if(target.kind==='installation'){installationConsumer??=options.settings.registerConsumer(registration);entry={consumer:installationConsumer,reading:0,evicted:false};}
    else{const existing=consumers.get(key);entry=existing??{consumer:options.settings.registerConsumer(registration),reading:0,evicted:false};if(!existing)consumers.set(key,entry);}
    entry.reading++;
    try{const snapshot=await entry.consumer.snapshot();const value=PromptsSettingsSchema.parse(snapshot.values);entry.consumer.reportApplied(snapshot,['/locale',...Object.keys(value.limits).map(key=>`/limits/${key}`)]);return {snapshot,value,consumer:entry.consumer};}
    finally{entry.reading--;if(entry.evicted&&!entry.reading)entry.consumer.close();}
  }
  function queryHead(database:Kysely<PromptDatabase>,id:string,locale:string,target:PromptScope){return database.selectFrom('promptHeads').selectAll().where('promptId','=',id).where('locale','=',locale).where('scopeKind','=',target.kind).where('scopeId','=',scopeId(target));}
  async function program(id:string,requestedLocale:string,limits:PromptLimits):Promise<PromptSource>{
    const item=descriptor(id);const locale=item.programFiles[requestedLocale]?requestedLocale:item.defaultLocale;
    const asset=await registry.load(item.programFiles[locale],'program',limits.sourceBytes);
    return {promptId:id,locale,scope:{kind:'installation',installationId},source:'program',digest:asset.digest,text:asset.text};
  }
  async function revision(database:Kysely<PromptDatabase>,id:string,locale:string,target:PromptScope,version:number,executable=true):Promise<PromptSource>{
    const row=await database.selectFrom('promptRevisions').select(['digest','sourceText','revision','paramsSchemaRevision']).where('promptId','=',id).where('locale','=',locale).where('scopeKind','=',target.kind).where('scopeId','=',scopeId(target)).where('revision','=',String(version)).executeTakeFirst();
    if(!row)throw problem('prompts.revision_not_found','not_found','指定Prompt修订不存在');
    if(executable&&row.paramsSchemaRevision!==descriptor(id).paramsSchemaRevision)throw problem('prompts.params_schema_incompatible','conflict','Prompt参数合同版本不再可执行');
    return {promptId:id,locale,scope:target,source:'override',revision:num(row.revision),digest:row.digest,text:row.sourceText};
  }
  async function select(database:Kysely<PromptDatabase>,binding:PromptBinding&{locale:string},target:PromptScope,limits:PromptLimits,executable=true):Promise<PromptSource>{
    descriptor(binding.promptId);
    // An explicit version is exact within its requested scope; it never retargets a parent version.
    if(binding.revision!==undefined)return revision(database,binding.promptId,binding.locale,target,binding.revision,executable);
    for(const current of target.kind==='subject'?[target,{kind:'installation' as const,installationId}]:[target]){
      const head=await queryHead(database,binding.promptId,binding.locale,current).executeTakeFirst();
      if(head?.activeRevision)return revision(database,binding.promptId,binding.locale,current,num(head.activeRevision),executable);
    }
    return program(binding.promptId,binding.locale,limits);
  }
  async function manifest(database:Kysely<PromptDatabase>,binding:PromptBinding&{locale:string},target:PromptScope,limits:PromptLimits,replacement?:PromptSource):Promise<PromptManifest>{
    const root=replacement??await select(database,binding,target,limits);
    const sources=await closure(root,catalog,async id=>{
      const source=await select(database,{promptId:id,locale:binding.locale},target,limits);
      return source;
    },limits);
    const item=descriptor(binding.promptId);const description=descriptions.get(item.id)!;
    const frozen={promptId:item.id,locale:root.locale,scope:target,sourceScope:root.scope,source:root.source,...(root.revision?{revision:root.revision}:{}),catalogRevision:description.catalogRevision,paramsSchemaRevision:item.paramsSchemaRevision,dependencies:sources.filter(source=>source.promptId!==root.promptId).map(({text,...source})=>source),engine:{name:'liquidjs' as const,version:ENGINE_VERSION,optionsDigest:optionsDigest(limits)}};
    const paramsSchemaDigest=jsonDigest(description.paramsSchema);
    return immutable({snapshot:{...frozen,ref:{id:uuidv7(),digest:jsonDigest({snapshot:frozen,sources,limits,paramsSchemaDigest})}},sources,limits,paramsSchemaDigest});
  }
  async function authorizeManifest(ctx:CallContext,value:PromptManifest){for(const source of value.sources)await authorize(ctx,{action:'snapshot',promptId:source.promptId,scope:source.scope});}
  async function persist(database:Kysely<PromptDatabase>,value:PromptManifest):Promise<PromptSnapshot>{
    const inserted=await database.insertInto('promptSnapshots').values({id:value.snapshot.ref.id,digest:value.snapshot.ref.digest,manifest:json(value)}).onConflict(oc=>oc.column('digest').doNothing()).returning('manifest').executeTakeFirst();
    return immutable((inserted??await database.selectFrom('promptSnapshots').select('manifest').where('digest','=',value.snapshot.ref.digest).executeTakeFirstOrThrow()).manifest.snapshot);
  }
  async function initialize(){
    for(const item of catalog.values())for(const locale of Object.keys(item.programFiles)){
      const root=await program(item.id,locale,promptsDefaults.limits);
      const sources=await closure(root,catalog,id=>program(id,locale,promptsDefaults.limits),promptsDefaults.limits);
      await validateExamples(item,sources,locale,promptsDefaults.limits);
    }
    await editor.watch();
  }
  async function mutate(ctx:CallContext,action:'save'|'activate'|'reset',input:PromptSave|PromptActivate|PromptReset):Promise<PromptChangeResult>{
    const target=scope(input.scope);const item=descriptor(input.promptId);
    if(!item.locales.includes(input.locale))throw problem('prompts.locale_not_supported','validation','Prompt目录未声明此语言');
    await authorize(ctx,{action,promptId:item.id,scope:target});
    const operationId=ctx.trace.operationId;
    if(!operationId||!ctx.actor.authorityRef)throw problem('prompts.context_required','forbidden','Prompt修改需要受信提交身份');
    const requestDigest=jsonDigest({action,input,humanId:ctx.actor.kind==='human'?ctx.actor.id:null,consumer:ctx.consumer,subjectId:ctx.subjectId??null,purpose:ctx.purpose,audience:ctx.audience});
    const receipt=await db.selectFrom('promptChangeReceipts').selectAll().where('operationId','=',operationId).executeTakeFirst();
    if(receipt){if(receipt.requestDigest!==requestDigest)throw problem('prompts.submission_conflict','conflict','同一提交包含不同Prompt修改');return immutable(receipt.result);}
    const cfg=await config(target);
    let planned:PromptManifest|undefined;
    if(action!=='reset'){
      planned=await db.transaction().setIsolationLevel('repeatable read').execute(async tx=>{
        const head=await queryHead(tx,item.id,input.locale,target).executeTakeFirst();
        const root=action==='save'?{promptId:item.id,locale:input.locale,scope:target,source:'override' as const,revision:num(head?.latestRevision)+1,digest:checkText((input as PromptSave).text,cfg.value.limits.sourceBytes),text:(input as PromptSave).text}:await revision(tx,item.id,input.locale,target,(input as PromptActivate).revision);
        return manifest(tx,{promptId:item.id,locale:input.locale},target,cfg.value.limits,root);
      });
      await authorizeManifest(ctx,planned);
      await validateExamples(item,planned.sources,input.locale,cfg.value.limits);
    }
    const committed=await db.transaction().execute(async tx=>{
      await sql`select pg_advisory_xact_lock(hashtextextended(${`prompts:operation:${operationId}`},0))`.execute(tx);
      const previous=await tx.selectFrom('promptChangeReceipts').selectAll().where('operationId','=',operationId).executeTakeFirst();
      if(previous){if(previous.requestDigest!==requestDigest)throw problem('prompts.submission_conflict','conflict','同一提交包含不同Prompt修改');return {result:previous.result,replay:true};}
      // Serialise short head edits so dependency validation cannot race a partial's activation.
      await sql`select pg_advisory_xact_lock(hashtextextended(${`prompts:heads:${installationId}`},0))`.execute(tx);
      const old=await queryHead(tx,item.id,input.locale,target).forUpdate().executeTakeFirst();const current=num(old?.revision);
      if(current!==input.expectedRevision)throw problem('prompts.revision_conflict','conflict','Prompt头已有新修订',{currentRevision:current});
      let latest=num(old?.latestRevision),active=old?.activeRevision?num(old.activeRevision):null,changed=false,snapshot:PromptSnapshot|undefined;
      if(action==='save'){
        const text=(input as PromptSave).text;const textDigest=checkText(text,cfg.value.limits.sourceBytes);if(!Number.isSafeInteger(latest+1))throw problem('prompts.invalid_revision','failed','Prompt修订超出有效范围');
        const root:PromptSource={promptId:item.id,locale:input.locale,scope:target,source:'override',revision:latest+1,digest:textDigest,text};
        const value=await manifest(tx,{promptId:item.id,locale:input.locale},target,cfg.value.limits,root);
        if(value.snapshot.ref.digest!==planned!.snapshot.ref.digest)throw problem('prompts.dependencies_changed','conflict','Prompt依赖已变化，请重新验证');
        const same=old?await tx.selectFrom('promptRevisions').select(['digest','paramsSchemaRevision']).where('promptId','=',item.id).where('locale','=',input.locale).where('scopeKind','=',target.kind).where('scopeId','=',scopeId(target)).where('revision','=',String(latest)).executeTakeFirst():undefined;
        if(same?.digest!==textDigest||same.paramsSchemaRevision!==item.paramsSchemaRevision){latest++;changed=true;await tx.insertInto('promptRevisions').values({id:uuidv7(),promptId:item.id,locale:input.locale,scopeKind:target.kind,scopeId:scopeId(target),revision:String(latest),sourceText:text,digest:textDigest,paramsSchemaRevision:item.paramsSchemaRevision,dependencies:json(value.snapshot.dependencies),origin:json({kind:'managed'}),actor:json(ctx.actor),reason:(input as PromptSave).reason??null}).execute();}
      }else if(action==='activate'){
        const root=await revision(tx,item.id,input.locale,target,(input as PromptActivate).revision);
        const value=await manifest(tx,{promptId:item.id,locale:input.locale},target,cfg.value.limits,root);
        if(value.snapshot.ref.digest!==planned!.snapshot.ref.digest)throw problem('prompts.dependencies_changed','conflict','Prompt依赖已变化，请重新验证');
        snapshot=await persist(tx,value);changed=active!==root.revision;active=root.revision!;
      }else {changed=active!==null;active=null;}
      const next=current+(changed?1:0);
      if(!Number.isSafeInteger(next))throw problem('prompts.invalid_revision','failed','Prompt头修订超出有效范围');
      if(changed){
        const values={promptId:item.id,locale:input.locale,scopeKind:target.kind,scopeId:scopeId(target),revision:String(next),latestRevision:String(latest),activeRevision:active===null?null:String(active)};
        if(old){const updated=await tx.updateTable('promptHeads').set(values).where('promptId','=',item.id).where('locale','=',input.locale).where('scopeKind','=',target.kind).where('scopeId','=',scopeId(target)).where('revision','=',old.revision).returning('revision').executeTakeFirst();if(!updated)throw problem('prompts.revision_conflict','conflict','Prompt头已有新修订');}
        else{const inserted=await tx.insertInto('promptHeads').values(values).onConflict(oc=>oc.columns(['promptId','locale','scopeKind','scopeId']).doNothing()).returning('revision').executeTakeFirst();if(!inserted)throw problem('prompts.revision_conflict','conflict','Prompt头已有新修订');}
      }
      const result:PromptChangeResult={promptId:item.id,locale:input.locale,scope:target,revision:next,latestRevision:latest,activeRevision:active,changed,...(snapshot?{snapshot}:{})};
      await tx.insertInto('promptChangeReceipts').values({operationId,action,promptId:item.id,locale:input.locale,scopeKind:target.kind,scopeId:scopeId(target),requestDigest,result:json(result),actorAuthorityRef:ctx.actor.authorityRef,consumer:json(ctx.consumer)}).execute();
      if(changed)await sql`select pg_notify('prompts_changed',${JSON.stringify({promptId:item.id,locale:input.locale,scope:target,revision:next})})`.execute(tx);
      return {result,replay:false};
    });
    if(!committed.replay&&committed.result.changed)for(const listener of listeners){try{listener({promptId:item.id,locale:input.locale,scope:target,revision:committed.result.revision});}catch{options.onError?.(problem('prompts.consumer_failed','failed','Prompt消费者通知失败').problem);}}
    return immutable(committed.result);
  }
  async function snapshot(ctx:CallContext,input:{binding:PromptBinding;scope:PromptScope}):Promise<PromptSnapshot>{
    input=parse(PromptSnapshotRequestSchema,input);const target=scope(input.scope);
    await authorize(ctx,{action:'snapshot',promptId:input.binding.promptId,scope:target});
    const cfg=await config(target);
    const binding={...input.binding,locale:input.binding.locale??cfg.value.locale};
    const planned=await db.transaction().setIsolationLevel('repeatable read').execute(tx=>manifest(tx,binding,target,cfg.value.limits));
    await authorizeManifest(ctx,planned);
    const result=await persist(db,planned);
    return result;
  }
  async function read(ctx:CallContext,input:PromptRead):Promise<PromptReadResult>{
    input=parse(PromptReadSchema,input);const target=scope(input.scope);
    await authorize(ctx,{action:'read',promptId:input.promptId,scope:target});
    const cfg=await config(target);
    const result=await db.transaction().setIsolationLevel('repeatable read').execute(async tx=>{
      const head=await queryHead(tx,input.promptId,input.locale,target).executeTakeFirst();
      const source=await select(tx,{promptId:input.promptId,locale:input.locale,revision:input.revision},target,cfg.value.limits,false);
      return immutable({promptId:input.promptId,locale:source.locale,requestedLocale:input.locale,scope:target,sourceScope:source.scope,source:source.source,...(source.revision?{sourceRevision:source.revision}:{}),digest:source.digest,text:source.text,revision:num(head?.revision),latestRevision:num(head?.latestRevision),activeRevision:head?.activeRevision?num(head.activeRevision):null});
    });
    await authorize(ctx,{action:'read',promptId:result.promptId,scope:result.sourceScope});
    return result;
  }
  async function render(ctx:CallContext,input:{snapshot:PromptSnapshotRef;params:unknown}){
    if(closed)throw problem('prompts.closed','unavailable','Prompt实例已关闭');
    const ref=parse(PromptSnapshotRefSchema,input.snapshot);
    const identity=await db.selectFrom('promptSnapshots').select(sql<PromptSnapshot>`manifest->'snapshot'`.as('snapshot')).where('id','=',ref.id).where('digest','=',ref.digest).executeTakeFirst();
    if(!identity)throw problem('prompts.snapshot_not_found','not_found','Prompt快照不存在');
    await authorize(ctx,{action:'render',promptId:identity.snapshot.promptId,scope:identity.snapshot.scope});
    const saved=await db.selectFrom('promptSnapshots').select('manifest').where('id','=',ref.id).where('digest','=',ref.digest).executeTakeFirstOrThrow();
    const item=descriptor(saved.manifest.snapshot.promptId);
    if(item.paramsSchemaRevision!==saved.manifest.snapshot.paramsSchemaRevision||jsonDigest(descriptions.get(item.id)!.paramsSchema)!==saved.manifest.paramsSchemaDigest)throw problem('prompts.params_schema_incompatible','conflict','该Prompt快照需要对应版本的领域参数合同');
    for(const source of saved.manifest.sources)await authorize(ctx,{action:'render',promptId:source.promptId,scope:source.scope});
    return immutable({...await renderSources(item,saved.manifest.sources,input.params,saved.manifest.limits),snapshot:saved.manifest.snapshot});
  }
  const api={
    initialize,read,snapshot,render,
    save:(ctx:CallContext,input:PromptSave)=>mutate(ctx,'save',parse(PromptSaveSchema,input)),
    activate:(ctx:CallContext,input:PromptActivate)=>mutate(ctx,'activate',parse(PromptActivateSchema,input)),
    reset:(ctx:CallContext,input:PromptReset)=>mutate(ctx,'reset',parse(PromptResetSchema,input)),
    async describe(ctx:CallContext,input:{owner?:string;scope?:PromptScope}={}){
      const target=scope(input.scope??{kind:'installation',installationId});
      await authorize(ctx,{action:'describe',scope:target});
      return immutable([...descriptions.values()].filter(item=>!input.owner||item.owner===input.owner));
    },
    async list(ctx:CallContext,input:{scope:PromptScope;cursor?:string;limit?:number}){
      const target=scope(input.scope);await authorize(ctx,{action:'list',scope:target});
      const limit=input.limit??50;if(!Number.isInteger(limit)||limit<1||limit>200)throw problem('prompts.invalid_page','validation','Prompt页大小必须为1至200');
      const entries=[...catalog.values()].flatMap(item=>item.locales.map(locale=>({promptId:item.id,locale}))).sort((a,b)=>`${a.promptId}:${a.locale}`.localeCompare(`${b.promptId}:${b.locale}`));
      let after='';if(input.cursor){try{const value=JSON.parse(Buffer.from(input.cursor,'base64url').toString());if(value.scope!==scopeKey(target)||typeof value.after!=='string')throw Error();after=value.after;}catch{throw problem('prompts.invalid_cursor','validation','Prompt游标无效');}}
      const selected=entries.filter(item=>`${item.promptId}:${item.locale}`>after).slice(0,limit+1);
      const heads=await db.selectFrom('promptHeads').selectAll().where('scopeKind','=',target.kind).where('scopeId','=',scopeId(target)).execute();
      const items=selected.slice(0,limit).map(item=>{const head=heads.find(head=>head.promptId===item.promptId&&head.locale===item.locale);return {...item,scope:target,revision:num(head?.revision),latestRevision:num(head?.latestRevision),activeRevision:head?.activeRevision?num(head.activeRevision):null};});
      return immutable({items,...(selected.length>limit?{nextCursor:Buffer.from(JSON.stringify({scope:scopeKey(target),after:`${items.at(-1)!.promptId}:${items.at(-1)!.locale}`})).toString('base64url')}:{})});
    },
    async history(ctx:CallContext,input:{promptId:string;locale:string;scope:PromptScope;cursor?:string;limit?:number}){
      const target=scope(input.scope);parse(PromptIdSchema,input.promptId);parse(PromptLocaleSchema,input.locale);await authorize(ctx,{action:'history',promptId:input.promptId,scope:target});
      const limit=input.limit??50;if(!Number.isInteger(limit)||limit<1||limit>200)throw problem('prompts.invalid_page','validation','Prompt页大小必须为1至200');
      const key=jsonDigest([input.promptId,input.locale,target]);let before:number|undefined;
      if(input.cursor){try{const value=JSON.parse(Buffer.from(input.cursor,'base64url').toString());if(value.key!==key||!Number.isSafeInteger(value.before)||value.before<1)throw Error();before=value.before;}catch{throw problem('prompts.invalid_cursor','validation','Prompt游标无效');}}
      let query=db.selectFrom('promptRevisions').selectAll().where('promptId','=',input.promptId).where('locale','=',input.locale).where('scopeKind','=',target.kind).where('scopeId','=',scopeId(target));if(before)query=query.where('revision','<',String(before));
      const rows=await query.orderBy('revision','desc').limit(limit+1).execute();const items=rows.slice(0,limit).map(row=>({id:row.id,promptId:row.promptId,locale:row.locale,scope:target,revision:num(row.revision),text:row.sourceText,digest:row.digest,paramsSchemaRevision:row.paramsSchemaRevision,reason:row.reason,recordedAt:row.recordedAt}));
      return immutable({items,...(rows.length>limit?{nextCursor:Buffer.from(JSON.stringify({key,before:items.at(-1)!.revision})).toString('base64url')}:{})});
    },
    async preview(ctx:CallContext,input:z.infer<typeof PromptPreviewSchema>){
      input=parse(PromptPreviewSchema,input);const target=scope(input.scope);await authorize(ctx,{action:'preview',promptId:input.binding.promptId,scope:target});
      if(input.unsavedText===undefined){const frozen=await snapshot(ctx,{binding:input.binding,scope:target});return {...await render(ctx,{snapshot:frozen.ref,params:input.params}),persisted:true as const};}
      const cfg=await config(target);const binding={...input.binding,locale:input.binding.locale??cfg.value.locale};const source:PromptSource={promptId:input.binding.promptId,locale:binding.locale,scope:target,source:'override',digest:checkText(input.unsavedText,cfg.value.limits.sourceBytes),text:input.unsavedText};
      const value=await db.transaction().setIsolationLevel('repeatable read').execute(tx=>manifest(tx,binding,target,cfg.value.limits,source));
      await authorizeManifest(ctx,value);
      return immutable({...await renderSources(descriptor(source.promptId),value.sources,input.params,cfg.value.limits),snapshot:null,persisted:false as const,draft:{digest:value.snapshot.ref.digest,promptId:source.promptId,locale:source.locale,dependencies:value.snapshot.dependencies,engine:value.snapshot.engine}});
    },
    subscribe(listener:(change:{promptId:string;locale:string;scope:PromptScope;revision:number})=>void){listeners.add(listener);return ()=>listeners.delete(listener);},
    close(){return closing??=(async()=>{await editor.close();closed=true;consumers.clear();installationConsumer?.close();listeners.clear();})();},
  };
  const editor=createEditor({installationId,settings:options.settings,overrideRoot:options.overrideRoot,registry,api,config,watchContext:options.watchContext,onError:options.onError});
  return {...api,exportFile:editor.exportFile,watch:editor.watch};
}
export type Prompts=ReturnType<typeof createPrompts>;
