import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { DBOS } from '@dbos-inc/dbos-sdk';
import type { LanguageModel } from 'ai';
import { ActorSchema,type Actor,type CallContext,type Ref,TimePointSchema,type TimePoint,type Policy } from '@nous-wave/contracts/common';
import type { ModelResource } from '@nous-wave/contracts/management';
import type {BlobMetadata,BlobResult} from '@nous-wave/contracts/materials';
import { KnowledgeKindSchema } from '@nous-wave/contracts/knowledge';
import { openDatabase } from './platform/database.js';
import { now,type Storage } from './platform/context.js';
import { authorize } from './platform/security.js';
import { fail } from './platform/errors.js';
import { createIdentity } from './identity/index.js';
import { createMaterials } from './materials/index.js';
import { createKnowledge } from './knowledge/index.js';
import { createOrganization } from './knowledge/organization.js';
import { createRetrieval } from './retrieval/index.js';
import { createCognition } from './cognition/index.js';
import { createResources } from './resources/index.js';
import { createManagement } from './management/index.js';
import { createModels } from './platform/models.js';
import { createInvocations } from './management/invocations.js';
import { createAuthentication } from './platform/auth.js';
import { ensureAuthSecret } from './platform/config.js';
import { startTelemetry } from './platform/telemetry.js';

export interface NousOptions {
  databaseUrl:string;dataRoot:string;installationId?:string;telemetry?:boolean;
  provider?:(resource:ModelResource,credential:string)=>Exclude<LanguageModel,string>;
  sourcePolicies?:(ctx:CallContext,roots:Ref[])=>Promise<Policy[]>;
  currentTime?:(ctx:CallContext)=>TimePoint|Promise<TimePoint>;
  backgroundMaintenance?:boolean;
  embedding?:{id:string;dimensions:number;embed(ctx:CallContext,texts:string[]):Promise<number[][]>};
}
export async function createNous(options:NousOptions) {
  const installationId=options.installationId??'nous-local';
  await mkdir(options.dataRoot,{recursive:true,mode:0o700});
  const telemetry=options.telemetry===false?undefined:startTelemetry(installationId);
  DBOS.setConfig({name:'nous-wave',systemDatabaseUrl:options.databaseUrl,applicationVersion:'spec-rebuild-1',enableOTLP:false});
  const database=await openDatabase(options.databaseUrl);
  const store:Storage={installationId,get db(){return DBOS.isInTransaction()?database.datasource.client:database.db;},transaction:async fn=>DBOS.isInTransaction()?fn():database.datasource.runTransaction(fn)};
  const owner=<T extends object>(extra:T):Storage&T=>Object.assign(Object.create(store),extra);
  const subjects=createIdentity(store);
  let knowledge:ReturnType<typeof createKnowledge>;
  let acceptBlobMetadata:(ctx:CallContext,input:BlobMetadata)=>Promise<BlobResult>;
  const {materials,encounters,sourceRepository}=createMaterials(owner({blobPath:join(options.dataRoot,'blobs'),readableSources:(ctx:CallContext,refs:Ref[])=>knowledge.sources.readable(ctx,refs),resolveSources:(ctx:CallContext,refs:Ref[])=>knowledge.sources.resolve(ctx,refs),inheritSources:(ctx:CallContext,refs:Ref[])=>knowledge.sources.inherit(ctx,refs),acceptBlob:(ctx:CallContext,input:BlobMetadata)=>acceptBlobMetadata(ctx,input)}));
  knowledge=createKnowledge(owner({sourceRepository,sourcePolicies:options.sourcePolicies}));
  const management=createManagement(owner({dataRoot:options.dataRoot}));
  const invocations=createInvocations({db:database.db,...management,sources:knowledge.sources});
  let modelPrepare:ReturnType<typeof createCognition>['contexts']['prepare'];
  const model=createModels({installationId,...management,invocations,materials,encounters,prepare:(ctx,input)=>modelPrepare(ctx,input),provider:options.provider});
  const retrieval=createRetrieval({installationId,indexPath:join(options.dataRoot,'indexes'),knowledge,materials,capabilities:async(ctx:CallContext)=>KnowledgeKindSchema.array().parse((await subjects.read(ctx)).capabilities),encounterInputs:encounters.inputs,authorize,embed:options.embedding,sourceTimes:materials.sourceTimes,relations:async(ctx,refs,limit,cutoff)=>{
    const relations=await knowledge.relations(ctx,refs.map(r=>r.id),{...cutoff,limit:Math.min(limit,200)});return relations.items.map(r=>({from:{product:'nous' as const,installationId,subjectId:ctx.subjectId,kind:'Knowledge',id:r.fromId},to:{product:'nous' as const,installationId,subjectId:ctx.subjectId,kind:'Knowledge',id:r.toId},relation:r.relation,weight:r.weight}));
  }});
  const operations={
    async inspect(ctx:CallContext,id:string) {authorize(ctx,'operations.read');const [status]=await DBOS.listWorkflows({workflowIDs:[id],loadOutput:true,loadInput:false});if(!status)fail('not_found','操作不存在');if(status.attributes?.subjectId!==ctx.subjectId&&!ctx.actor.grants.includes('installation.manage')&&!ctx.actor.grants.includes('*'))fail('forbidden','操作不属于授权主体');const mapping:Record<string,string>={ENQUEUED:'queued',DELAYED:'waiting',PENDING:'running',SUCCESS:'completed',ERROR:'failed',CANCELLED:'cancelled',MAX_RECOVERY_ATTEMPTS_EXCEEDED:'failed'};return {operationId:id,status:mapping[status.status]??'waiting',result:status.output,error:status.error,createdAt:status.createdAt,updatedAt:status.updatedAt};},
    async trace(ctx:CallContext,id:string){await operations.inspect(ctx,id);return {operationId:id,steps:await DBOS.listWorkflowSteps(id)};},
    async cancel(ctx:CallContext,id:string){authorize(ctx,'operations.cancel');await operations.inspect(ctx,id);await DBOS.cancelWorkflow(id);return operations.inspect(ctx,id);},
    async resume(ctx:CallContext,id:string){authorize(ctx,'operations.resume');await operations.inspect(ctx,id);await DBOS.resumeWorkflow(id);return operations.inspect(ctx,id);},
  };
  const currentTime=async(ctx:CallContext)=>{
    const read=async()=>{if(options.currentTime)return TimePointSchema.parse(await options.currentTime(ctx));const binding=await database.db.selectFrom('subjectBindings').select('id').where('subjectId','=',ctx.subjectId).limit(1).executeTakeFirst();const experience=await database.db.selectFrom('experienceItems').select('clockId').where('subjectId','=',ctx.subjectId).orderBy('recordedAt','desc').limit(1).executeTakeFirst();if(binding||experience&&experience.clockId!=='real')fail('dependency_unavailable','主体时间需要可信宿主时钟提供者');return now();};
    return DBOS.isWithinWorkflow()?DBOS.runStep(read,{name:'subject.currentTime',retriesAllowed:false}):read();
  };
  const cognition=createCognition(owner({knowledge,materials,encounters,retrieval,model,currentTime,settingsRevisions:async(ctx:CallContext,role:string)=>Object.fromEntries((await management.settings.read(ctx,role)).overrides.map(r=>[`${r.scope}:${r.scopeId}`,r.revision])),capabilities:async(ctx:CallContext)=>KnowledgeKindSchema.array().parse((await subjects.read(ctx)).capabilities),operationControl:{inspect:operations.inspect,pause:operations.cancel}}));
  const resources=createResources(owner({materials}));
  const refresh=async(ctx:CallContext,views?:Parameters<typeof retrieval.refresh>[1])=>DBOS.isWithinWorkflow()?DBOS.runStep(()=>retrieval.refresh(ctx,views),{name:'retrieval.refresh',retriesAllowed:false}):retrieval.refresh(ctx,views);
  const organization=createOrganization(owner({knowledge,materials,encounters,model,refresh}));
  function workflow<I,O>(name:string,fn:(ctx:CallContext,input:I)=>Promise<O>,queueName?:string) {
    const registered=DBOS.registerWorkflow(fn,{name});
    const start=async(ctx:CallContext,input:I)=>{
      const id=`${name}:${ctx.traceContext.operationId}`;
      const [prior]=await DBOS.listWorkflows({workflowIDs:[id],loadInput:true,loadOutput:false});
      if(prior?.input){const [oldCtx,oldInput]=prior.input as [CallContext,unknown];if(oldCtx.actor.id!==ctx.actor.id||oldCtx.subjectId!==ctx.subjectId||JSON.stringify(oldInput)!==JSON.stringify(input))fail('conflict','客户端提交 ID 已对应另一输入',{operationId:id});}
      const handle=await DBOS.startWorkflow(registered,{workflowID:id,queueName,enqueueOptions:queueName?{queuePartitionKey:ctx.subjectId}:undefined,workflowAttributes:{subjectId:ctx.subjectId,actorId:ctx.actor.id,operation:name,traceId:ctx.traceContext.traceId}})(ctx,input);
      return handle;
    };
    return Object.assign(async(ctx:CallContext,input:I):Promise<O>=>DBOS.isWithinWorkflow()?registered(ctx,input):(await start(ctx,input)).getResult(),{start});
  }
  const capture=workflow('materials.capture',materials.capture);
  const publishMaterial=workflow('materials.publish',materials.publish);
  const acceptBlob=workflow('materials.acceptBlob',materials.acceptBlob);acceptBlobMetadata=acceptBlob;
  const accept=workflow('encounters.accept',encounters.accept);
  const prepare=workflow('contexts.prepare',cognition.contexts.prepare);
  modelPrepare=prepare;
  const createKnowledgeOperation=workflow('knowledge.create',async(ctx,input:Parameters<typeof knowledge.create>[1])=>{const view=await knowledge.create(ctx,input);await refresh(ctx,[view]);return view;});
  const reviseKnowledgeOperation=workflow('knowledge.revise',async(ctx,input:Parameters<typeof knowledge.revise>[1])=>{const view=await knowledge.revise(ctx,input);await refresh(ctx,[view]);return view;});
  const runGrowth=workflow('growth.run',cognition.growth.run,'nous-growth');
  const resolveGrowth=workflow('growth.resolve',cognition.growth.resolve,'nous-growth');
  const applyGrowth=workflow('growth.apply',cognition.growth.apply,'nous-growth');
  const previewSeed=workflow('seeds.preview',cognition.seeds.preview,'nous-growth');
  const applySeed=workflow('seeds.apply',cognition.seeds.apply,'nous-growth');
  const runMaintenance=workflow('maintenance.run',cognition.maintenance.run,'nous-growth');
  const readResource=workflow('resources.read',resources.read);
  const organizeEpisodes=workflow('episodes.organize',organization.episodes.organize);
  const writeJournal=workflow('journals.write',organization.journals.write,'nous-growth');
  const mergeConcepts=workflow('concepts.merge',organization.concepts.merge);
  const splitConcepts=workflow('concepts.split',organization.concepts.split);
  const evaluateMotivation=workflow('motivation.evaluate',organization.motivation.evaluate,'nous-growth');
  await DBOS.launch();
  await DBOS.registerQueue('nous-growth',{partitionConcurrency:1,workerConcurrency:2});
  let authentication:ReturnType<typeof createAuthentication>|undefined;
  async function getAuthentication(baseURL='http://127.0.0.1:4333'){authentication??=createAuthentication(database.pool,await ensureAuthSecret(options.dataRoot),baseURL);return authentication;}
  return {
    installationId,subjects,
    materials:{...materials,capture,publish:publishMaterial,acceptBlob},encounters:{...encounters,accept},
    knowledge:{...knowledge,create:createKnowledgeOperation,revise:reviseKnowledgeOperation,query:retrieval.query,expand:retrieval.expand,explainQuery:retrieval.explain},
    contexts:{...cognition.contexts,prepare},growth:{...cognition.growth,run:runGrowth,apply:applyGrowth,resolve:resolveGrowth},seeds:{...cognition.seeds,preview:previewSeed,apply:applySeed},maintenance:{...cognition.maintenance,run:runMaintenance},feedback:cognition.feedback,
    episodes:{...organization.episodes,organize:organizeEpisodes},journals:{write:writeJournal},concepts:{merge:mergeConcepts,split:splitConcepts},motivation:{...organization.motivation,evaluate:evaluateMotivation},persona:organization.persona,
    resources:{...resources,read:readResource},...management,invocations,operations,
    authentication:getAuthentication,
    credentials:{async issue(ctx:CallContext,input:{name:string;actor:Actor}){authorize(ctx,'installation.manage');if(ctx.actor.kind!=='human')fail('forbidden','凭据签发需要管理员身份');return (await getAuthentication()).createKey(ctx.actor.id,ActorSchema.parse(input.actor),input.name);}},
    async doctor(){await database.db.selectFrom('subjects').select('id').limit(1).execute();return {product:'nous-wave',installationId,database:'available',runtime:'available',time:now()};},
    async close(){await retrieval.close();await DBOS.shutdown();await telemetry?.close();},
  };
}
export type Nous=Awaited<ReturnType<typeof createNous>>;
export { makeContext } from './platform/context.js';

export { loadBootstrap, secretFile } from './platform/config.js';
export { DomainError } from './platform/errors.js';
export { span } from './platform/telemetry.js';
