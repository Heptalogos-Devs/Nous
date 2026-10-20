import { createHash } from 'node:crypto';
import { sql,type Kysely } from 'kysely';
import { z } from 'zod';
import { RefSchema } from '@nous-wave/contracts/common';
import { InvocationPrepareSchema,InvocationCompletionSchema } from '@nous-wave/contracts/management';
import type { CallContext } from '@nous-wave/contracts/common';
import { canonicalRefKey } from '@nous-wave/contracts/common';
import type { ModelResource,Settings } from '@nous-wave/contracts/management';
import type { Database } from '../platform/database.js';
import { authorize } from '../platform/security.js';
import { fail } from '../platform/errors.js';
import { json,now } from '../platform/context.js';
import type {BudgetConstraint} from './settings.js';
import {ModelResourceSchema} from '@nous-wave/contracts/management';

export interface InvocationDependencies {
  db:Kysely<Database>;settings:{read(ctx:CallContext,role?:string):Promise<{effective:Settings;constraints:BudgetConstraint[]}>};
  models:{read(ctx:CallContext,id:string):Promise<ModelResource>};
  sources:{resolve(ctx:CallContext,refs:z.infer<typeof RefSchema>[]):Promise<unknown>};
}
export function createInvocations(deps:InvocationDependencies) {
  async function inspect(ctx:CallContext,id:string) {authorize(ctx,'models.invoke');const row=await deps.db.selectFrom('modelInvocations').selectAll().where('id','=',id).where('subjectId','=',ctx.subjectId).executeTakeFirst();if(!row)fail('not_found','模型调用不存在');return row;}
  return {
    inspect,
    async prepare(ctx:CallContext,input:unknown) {
      authorize(ctx,'models.invoke');const request=InvocationPrepareSchema.parse(input);await deps.sources.resolve(ctx,request.inputRefs);
      const [resource,{effective:settings,constraints}]=await Promise.all([deps.models.read(ctx,request.modelResourceId),deps.settings.read(ctx,request.role)]);
      if(request.inputTokensEstimate+request.maxOutputTokens>resource.contextLimit||request.maxOutputTokens>resource.outputLimit)fail('budget_exhausted','模型输入与输出预留超过资源上限');
      if(settings.budget.dailyAmount!==null&&!resource.pricing&&settings.budget.perCallAmount===null)fail('budget_exhausted','严格金额预算需要模型价格或明确单次费用上限');
      const reservedCost=resource.pricing?(request.inputTokensEstimate*resource.pricing.inputPerMillion+request.maxOutputTokens*resource.pricing.outputPerMillion)/1e6:settings.budget.perCallAmount??0;
      if(settings.budget.perCallAmount!==null&&reservedCost>settings.budget.perCallAmount)fail('budget_exhausted','模型费用预留超过单次上限');
      const requestHash=createHash('sha256').update(JSON.stringify([request.role,request.modelResourceId,request.inputTokensEstimate,request.maxOutputTokens,request.inputRefs.map(canonicalRefKey),request.nativeRunRef?canonicalRefKey(request.nativeRunRef):null])).digest('hex');
      return deps.db.transaction().execute(async db=>{
        await sql`select pg_advisory_xact_lock(hashtext('nous:model-budget'))`.execute(db);
        const existing=await db.selectFrom('modelInvocations').selectAll().where('id','=',request.invocationId).forUpdate().executeTakeFirst();
        if(existing){if(existing.subjectId!==ctx.subjectId||existing.requestHash!==requestHash)fail('conflict','同一模型调用身份已有不同输入');return {invocationId:existing.id,status:existing.status,reservedCost:existing.reservedCost};}
        const start=new Date();start.setUTCHours(0,0,0,0);
        for(const constraint of constraints){
          let query=db.selectFrom('modelInvocations').select(['actualCost','reservedCost','status','request','usage']).where('startedAt','>=',start.toISOString()).where('status','!=','not_sent');
          if(constraint.scope!=='installation')query=query.where('subjectId','=',ctx.subjectId);
          if(constraint.scope==='role')query=query.where('role','=',request.role);
          const previous=await query.execute();const limits=constraint.limits;
          const spent=previous.reduce((sum,row)=>sum+(row.actualCost??row.reservedCost),0);
          const active=previous.filter(row=>['prepared','dispatched','unknown'].includes(row.status)).length;
          let inputTokens=0,outputTokens=0;
          for(const row of previous){const used=row.usage as Record<string,unknown>,plan=InvocationPrepareSchema.parse(row.request);inputTokens+=typeof used.inputTokens==='number'?used.inputTokens:plan.inputTokensEstimate;outputTokens+=typeof used.outputTokens==='number'?used.outputTokens:plan.maxOutputTokens;}
          if(limits.dailyCalls!==undefined&&previous.length>=limits.dailyCalls||limits.concurrentCalls!==undefined&&active>=limits.concurrentCalls||limits.dailyAmount!==undefined&&limits.dailyAmount!==null&&spent+reservedCost>limits.dailyAmount||limits.dailyInputTokens!==undefined&&limits.dailyInputTokens!==null&&inputTokens+request.inputTokensEstimate>limits.dailyInputTokens||limits.dailyOutputTokens!==undefined&&limits.dailyOutputTokens!==null&&outputTokens+request.maxOutputTokens>limits.dailyOutputTokens)fail('budget_exhausted',`${constraint.scope} 本日模型预算或并发额度不足`);
        }
        await db.insertInto('modelInvocations').values({id:request.invocationId,subjectId:ctx.subjectId,role:request.role,modelResourceId:resource.id,status:'prepared',reservedCost,actualCost:null,usage:json({}),providerRequestId:null,inputRefs:json(request.inputRefs),workflowId:ctx.traceContext.operationId,traceId:ctx.traceContext.traceId,startedAt:now().at,completedAt:null,request:json({...request,resourceSnapshot:resource,settingsConstraints:constraints}),requestHash,completionHash:null}).execute();
        return {invocationId:request.invocationId,status:'prepared',reservedCost};
      });
    },
    async dispatch(ctx:CallContext,id:string) {
      authorize(ctx,'models.invoke');
      const prepared=await inspect(ctx,id);const request=InvocationPrepareSchema.parse(prepared.request);await deps.sources.resolve(ctx,request.inputRefs);
      const updated=await deps.db.updateTable('modelInvocations').set({status:'dispatched'}).where('id','=',id).where('subjectId','=',ctx.subjectId).where('status','=','prepared').returning('id').executeTakeFirst();
      if(!updated)fail('effect_unknown','该调用已有派发记录，需复用原提供方结果或明确处理',{operationId:ctx.traceContext.operationId});
      return {invocationId:id,status:'dispatched'};
    },
    async complete(ctx:CallContext,id:string,input:unknown) {
      authorize(ctx,'models.invoke');const completion=InvocationCompletionSchema.parse(input);const invocation=await inspect(ctx,id);const snapshot=(invocation.request as {resourceSnapshot?:unknown}).resourceSnapshot;const resource=ModelResourceSchema.parse(snapshot);
      const completionHash=createHash('sha256').update(JSON.stringify([completion.status,completion.inputTokens,completion.outputTokens,completion.cacheReadTokens,completion.cacheWriteTokens,completion.providerRequestId,completion.outputRef?canonicalRefKey(completion.outputRef):null])).digest('hex');
      const actualCost=completion.status==='not_sent'?0:completion.status==='completed'&&resource.pricing&&completion.inputTokens!==undefined&&completion.outputTokens!==undefined?(completion.inputTokens*resource.pricing.inputPerMillion+completion.outputTokens*resource.pricing.outputPerMillion)/1e6:null;
      return deps.db.transaction().execute(async db=>{
        const current=await db.selectFrom('modelInvocations').selectAll().where('id','=',id).where('subjectId','=',ctx.subjectId).forUpdate().executeTakeFirstOrThrow();
        if(current.completionHash&&current.status!=='unknown'){if(current.completionHash!==completionHash)fail('conflict','该模型调用已有不同完成结果');return current;}
        await db.updateTable('modelInvocations').set({status:completion.status,usage:json({...completion,cost:{amount:actualCost,source:completion.status==='not_sent'?'not_sent':actualCost===null?'unknown':'resource_estimate',currency:resource.pricing?.currency??null}}),providerRequestId:completion.providerRequestId??null,actualCost,reservedCost:actualCost===null?current.reservedCost:0,completedAt:completion.status==='unknown'?null:now().at,completionHash}).where('id','=',id).execute();
        return db.selectFrom('modelInvocations').selectAll().where('id','=',id).executeTakeFirstOrThrow();
      });
    },
  };
}
