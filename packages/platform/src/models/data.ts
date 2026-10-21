// Immutable physical-call history travels with its product data closure.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import {canonicalize} from 'json-canonicalize';
import {SubjectIdSchema,type CallContext,type Ref} from '@nous-wave/contracts/common';
import {ModelDataBundleSchema} from '@nous-wave/contracts/models';
import {sql,type Kysely,type Selectable} from 'kysely';
import type {ModelsOptions} from './types.js';
import type {ModelsDatabase,ModelInvocationRow} from './migration.js';
import {problem} from '../errors.js';
export {ModelDataBundleSchema,type ModelDataBundle} from '@nous-wave/contracts/models';
export function createModelDataLifecycle(options:Pick<ModelsOptions,'db'|'product'|'installationId'|'authorize'|'data'>){
 const within=<T>(client:Kysely<any>|undefined,use:(db:Kysely<ModelsDatabase>)=>Promise<T>)=>{if(client){if(!client.isTransaction)throw problem('models.transaction_required','validation','模型生命周期client必须是当前事务');return use(client);}return options.db.transaction().setIsolationLevel('repeatable read').execute(use);};
 const reference=(row:{id:string;subjectId:string|null}):Ref=>({product:options.product,installationId:options.installationId,kind:'ModelInvocation',id:row.id,...(row.subjectId?{subjectId:row.subjectId}:{})});
 function scope(ctx:CallContext,subjectId:string|null){if(ctx.subjectId!==undefined&&ctx.subjectId!==subjectId)throw problem('models.data_scope','forbidden','模型调用数据不属于当前主体范围');}
 function immutableIdentity(row:Selectable<ModelInvocationRow>){return canonicalize({id:row.id,logicalOperationId:row.logicalOperationId,physicalOrdinal:row.physicalOrdinal,attemptNo:row.attemptNo,subjectId:row.subjectId,consumer:row.consumer,purpose:row.purpose,audience:row.audience,authorityRef:row.authorityRef,requestDigest:row.requestDigest,configSnapshot:row.configSnapshot,inputSources:row.inputSources,recordedAt:row.recordedAt});}
 function values(row:Selectable<ModelInvocationRow>){
  const json=<T>(value:T)=>sql<T>`${JSON.stringify(value)}::jsonb`;
  return {...row,consumer:json(row.consumer),audience:json(row.audience),configSnapshot:json(row.configSnapshot),preparation:json(row.preparation),
   inputSources:json(row.inputSources),semanticInputRefs:json(row.semanticInputRefs),outputRefs:json(row.outputRefs),
   observedCapture:json(row.observedCapture),formedCapture:json(row.formedCapture),tokenEstimate:json(row.tokenEstimate),
   usage:json(row.usage),rawUsage:json(row.rawUsage),budgetSnapshot:json(row.budgetSnapshot),budgetKeys:json(row.budgetKeys),problem:json(row.problem),nativeIdentity:json(row.nativeIdentity)};
 }
 return {
  export(ctx:CallContext,input:{subjectId?:string},client?:Kysely<any>){return within(client,async db=>{
   const subjectId=input.subjectId??ctx.subjectId??null;scope(ctx,subjectId);if(subjectId===null)await options.authorize(ctx,'installation.manage',undefined,db);await options.authorize(ctx,'data.export',undefined,db);
   let query=db.selectFrom('modelInvocations').selectAll();if(subjectId!==null)query=query.where('subjectId','=',subjectId);
   const calls=await query.orderBy('recordedAt').orderBy('id').execute();for(const call of calls)await options.authorize(ctx,'data.export',reference(call),db);
   return ModelDataBundleSchema.parse({formatVersion:1,sourceProduct:options.product,sourceInstallationId:options.installationId,subjectId,calls});
  });},
  import(ctx:CallContext,input:unknown,client?:Kysely<any>){const bundle=ModelDataBundleSchema.parse(input);if(bundle.sourceProduct!==options.product||bundle.sourceInstallationId!==options.installationId)throw problem('models.restore_identity_required','conflict','调用事实恢复需要原产品安装身份，不能改写原付费调用和来源引用');scope(ctx,bundle.subjectId);if(bundle.subjectId!==null&&bundle.calls.some(call=>call.subjectId!==bundle.subjectId))throw problem('models.data_scope','validation','主体包包含其他主体的调用');return within(client,async db=>{
   if(bundle.subjectId===null)await options.authorize(ctx,'installation.manage',undefined,db);await options.authorize(ctx,'data.import',undefined,db);let restored=0,retained=0;
   for(const incoming of bundle.calls){
    await options.authorize(ctx,'data.import',reference(incoming),db);
    const logical=await db.selectFrom('modelInvocations').selectAll().where('logicalOperationId','=',incoming.logicalOperationId).where('physicalOrdinal','=',incoming.physicalOrdinal).where('attemptNo','=',incoming.attemptNo).forUpdate().executeTakeFirst();
    if(logical&&logical.id!==incoming.id)throw problem('models.restore_conflict','conflict','原逻辑调用身份已对应不同物理调用');
    const existing=logical??await db.selectFrom('modelInvocations').selectAll().where('id','=',incoming.id).forUpdate().executeTakeFirst();
    if(existing&&immutableIdentity(existing)!==immutableIdentity(incoming))throw problem('models.restore_conflict','conflict','原物理调用的不可变输入或身份发生冲突');
    if(existing&&['prepared','dispatched'].includes(existing.status))throw problem('models.active_calls','conflict','恢复前请先停止或核实仍在使用的实际模型调用');
    const known=(status:string)=>status==='completed'||status==='not_sent';
    if(existing&&known(existing.status)){
     if(known(incoming.status)&&(existing.status!==incoming.status||existing.completionDigest!==incoming.completionDigest))throw problem('models.restore_outcome_conflict','conflict','同一物理调用存在不同已确认结果');retained++;continue;
    }
    const row:Selectable<ModelInvocationRow>={...incoming,...(['prepared','dispatched'].includes(incoming.status)?{status:'unknown' as const}:{} )};
    if(existing){if(!known(incoming.status)){retained++;continue;}await db.updateTable('modelInvocations').set(values(row)).where('id','=',row.id).execute();}
    else await db.insertInto('modelInvocations').values(values(row)).execute();restored++;
   }
   return {restored,retained};
  });},
  deleteSubject(ctx:CallContext,input:{subjectId:string},client?:Kysely<any>){SubjectIdSchema.parse(input.subjectId);scope(ctx,input.subjectId);return within(client,async db=>{
   await options.authorize(ctx,'data.purge',undefined,db);const rows=await db.selectFrom('modelInvocations').select(['id','subjectId','status']).where('subjectId','=',input.subjectId).execute();for(const row of rows){await options.authorize(ctx,'data.purge',reference(row),db);if(['prepared','dispatched','unknown'].includes(row.status))throw problem('models.unresolved_calls','conflict','请先停止或核实该主体尚未结清的实际模型调用');}
   await db.deleteFrom('modelInvocations').where('subjectId','=',input.subjectId).execute();return {deleted:rows.length};
  });},
 };
}
