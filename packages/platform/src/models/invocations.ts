// Exact-money admission and immutable request identity adapted from the current
// Nous ModelInvocation owner. No workflow progress is stored in these tables.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import {v7 as uuid} from 'uuid';
import {sql,type Kysely,type Selectable} from 'kysely';
import {Decimal} from 'decimal.js';
import {Temporal} from '@js-temporal/polyfill';
import {canonicalize} from 'json-canonicalize';
import {ModelCallViewSchema,ModelInvocationFilterSchema,NormalizedUsageSchema,ModelReconcileSchema,NativeOriginSchema,type InvocationSnapshot,type ModelInvocationIdentity,type ModelCallView,type TokenEstimate,type NormalizedUsage,type ModelPreparation} from '@nous-wave/contracts/models';
import {TimeCaptureReceiptSchema,type TimeCaptureReceipt} from '@nous-wave/contracts/time';
import type {CallContext,Money,Problem,Ref} from '@nous-wave/contracts/common';
import {digest,immutable,parse} from '../settings/catalog.js';
import {readReal,parseInstant} from '../time/index.js';
import {problem} from '../errors.js';
import type {ModelsDatabase} from './migration.js';
import type {ModelsOptions,BudgetConstraint,TerminalMetadata} from './types.js';
import {reserveCost,modelCost} from './usage.js';
type Row=Selectable<ModelsDatabase['modelInvocations']>;
const json=<T>(value:T)=>sql<T>`${JSON.stringify(value)}::jsonb`;
const Exact=Decimal.clone({precision:1024});
const amount=(value:string|null,currency:string|null):Money|null=>value===null||currency===null?null:{amount:value,currency};
function identity(row:Row):ModelInvocationIdentity{return {invocationId:row.id,logicalOperationId:row.logicalOperationId,physicalOrdinal:row.physicalOrdinal,attemptNo:row.attemptNo,...(row.workflowId?{workflowId:row.workflowId}:{}),...(row.stepNo===null?{}:{stepNo:row.stepNo}),...(row.batchNo===null?{}:{batchNo:row.batchNo})};}
function view(row:Row):ModelCallView {
 return parse(ModelCallViewSchema,{identity:identity(row),authorityRef:row.authorityRef,...(row.nativeIdentity?{native:parse(NativeOriginSchema,row.nativeIdentity)}:{}),traceId:row.traceId,purpose:row.purpose,audience:row.audience,...(row.subjectId?{subjectId:row.subjectId}:{}),consumer:row.consumer,roleId:row.roleId,kind:row.modelKind,status:row.status,snapshot:row.configSnapshot,...(row.preparation?{preparation:row.preparation}:{}),inputRefs:row.inputSources,outputRefs:row.outputRefs,...(row.observedCapture?{observed:row.observedCapture}:{}),...(row.formedCapture?{formed:row.formedCapture}:{}),...(row.usage?{usage:row.usage}:{}),...(row.providerRequestId?{providerRequestId:row.providerRequestId}:{}),reservedCost:amount(row.reservedCost,row.currency),actualCost:amount(row.actualCost,row.currency),...(row.problem?{problem:row.problem}:{}),recordedAt:parseInstant(row.recordedAt),...(row.completedAt?{completedAt:parseInstant(row.completedAt)}:{})});
}
export interface InvocationPreparation {identity:Omit<ModelInvocationIdentity,'invocationId'>&{invocationId?:string};snapshot:InvocationSnapshot;constraints:BudgetConstraint[];inputRefs:Ref[];estimate:TokenEstimate;maxOutputTokens:number;observed?:TimeCaptureReceipt;semanticInputRefs?:Ref[];native?:unknown;preparation?:ModelPreparation}
export function createInvocationOwner(options:ModelsOptions){
 const ref=(id:string,subjectId?:string|null):Ref=>({product:options.product,installationId:options.installationId,...(subjectId?{subjectId}:{}),kind:'ModelInvocation',id});
 const modelRef=(snapshot:InvocationSnapshot):Ref=>({product:options.product,installationId:options.installationId,kind:'ModelResource',id:snapshot.actualResource.id,revision:snapshot.actualResource.revision});
 async function selected(ctx:CallContext,id:string,database=options.db){const row=await database.selectFrom('modelInvocations').selectAll().where('id','=',id).executeTakeFirst();if(!row)throw problem('models.invocation_not_found','not_found','模型调用不存在');await options.authorize(ctx,'models.invocations.read',ref(id,row.subjectId),database);return row;}
 async function own(ctx:CallContext,id:string,database=options.db){const row=await database.selectFrom('modelInvocations').selectAll().where('id','=',id).executeTakeFirst();if(!row)throw problem('models.invocation_not_found','not_found','模型调用不存在');await options.authorize(ctx,'models.invoke',modelRef(row.configSnapshot),database);if(row.subjectId!==(ctx.subjectId??null)||canonicalize(row.consumer)!==canonicalize(ctx.consumer))throw problem('models.invocation_scope','forbidden','调用主体或消费者不匹配');return row;}
 async function fact(id:string,database=options.db){const row=await database.selectFrom('modelInvocations').selectAll().where('id','=',id).executeTakeFirst();if(!row)throw problem('models.invocation_not_found','not_found','模型调用不存在');return row;}
 async function prepare(ctx:CallContext,input:InvocationPreparation){
  const requestDigest=digest({...input,identity:{...input.identity,invocationId:undefined},constraints:undefined,observed:undefined,semanticInputRefs:undefined,native:input.native??null});
  const keys=input.constraints.map(item=>item.key);
  const allocation=input.snapshot.budgetAllocation;
  const constraints=[...input.constraints,...(allocation?[{key:`allocation:${allocation.receiptDigest}`,budget:{...allocation.limits}}]:[])];
  const budgetKeys=[...new Set(constraints.map(item=>item.key))].sort();
  let reserved=reserveCost(input.snapshot.pricing,input.estimate,input.maxOutputTokens);
  const explicitAmounts=constraints.flatMap(item=>item.budget.perCallAmount?[item.budget.perCallAmount]:[]);
  if(!reserved&&explicitAmounts.length){const currencies=new Set(explicitAmounts.map(value=>value.currency));if(currencies.size!==1)throw problem('models.budget_currency','budget','费用预留的币种不一致');reserved=explicitAmounts.reduce((a,b)=>new Exact(a.amount).lte(b.amount)?a:b);}
  const at=readReal().at;
  return options.db.transaction().execute(async db=>{
   await sql`select pg_advisory_xact_lock(hashtextextended(${`models:operation:${input.identity.logicalOperationId}:${input.identity.physicalOrdinal}:${input.identity.attemptNo}`},0))`.execute(db);
   const existing=await db.selectFrom('modelInvocations').selectAll().where('logicalOperationId','=',input.identity.logicalOperationId).where('physicalOrdinal','=',input.identity.physicalOrdinal).where('attemptNo','=',input.identity.attemptNo).executeTakeFirst();
   if(existing){if(existing.requestDigest!==requestDigest||existing.subjectId!==(ctx.subjectId??null)||canonicalize(existing.consumer)!==canonicalize(ctx.consumer)||existing.purpose!==ctx.purpose||canonicalize(existing.audience)!==canonicalize(ctx.audience)||input.identity.invocationId&&input.identity.invocationId!==existing.id)throw problem('models.submission_conflict','conflict','同一物理调用身份包含不同输入');if(existing.status!=='prepared')return view(existing);}
   await options.authorize(ctx,'models.invoke',modelRef(input.snapshot),db);
   if(allocation&&(allocation.target.product!==options.product||allocation.target.installationId!==options.installationId||allocation.target.subjectId!==ctx.subjectId||canonicalize(allocation.target.consumer)!==canonicalize(ctx.consumer)||allocation.expiresAt.at<=at))throw problem('models.allocation_scope','forbidden','子额度的目标、消费者或期限不匹配');
   const resource=input.snapshot.definition.managedBy==='sdk'?input.snapshot.definition.definition:input.snapshot.definition.binding;
   if(input.snapshot.policy.kind==='language'&&(!resource.contextLimit||!resource.outputLimit||input.estimate.value+input.maxOutputTokens>resource.contextLimit||input.maxOutputTokens>resource.outputLimit))throw problem('models.context_budget','budget','模型窗口、输出上限或必要元数据不满足本次输入');
   for(const key of budgetKeys)await sql`select pg_advisory_xact_lock(hashtextextended(${`models:budget:${key}`},0))`.execute(db);
   for(const constraint of constraints){
    const bounds=constraint.budget;const moneyLimits=[bounds.totalAmount,bounds.dailyAmount,bounds.perCallAmount].filter((value):value is Money=>!!value);
    if(moneyLimits.some(value=>!reserved||value.currency!==reserved.currency))throw problem('models.budget_currency','budget','金额上限需要明确匹配币种的费用预留');
    if(bounds.perCallAmount&&reserved&&new Exact(reserved.amount).gt(bounds.perCallAmount.amount))throw problem('models.per_call_budget','budget','本次模型费用预留超过单次上限');
    const base=()=>{let query=db.selectFrom('modelInvocations').where(sql<boolean>`budget_keys ? ${constraint.key}`).where('status','!=','not_sent');if(existing)query=query.where('id','!=',existing.id);return query;};
    let window=base();if(bounds.window)window=window.where('recordedAt','>=',Temporal.Instant.from(at).subtract({milliseconds:bounds.window.durationMs}).toString({fractionalSecondDigits:6}));
    const counts=await window.select([sql<string>`count(*)`.as('calls'),sql<string>`coalesce(sum(coalesce(actual_tokens,(token_estimate->>'value')::bigint+max_output_tokens)),0)`.as('tokens')]).executeTakeFirstOrThrow();
    const active=await base().where('status','in',['prepared','dispatched','unknown']).select(sql<string>`count(*)`.as('calls')).executeTakeFirstOrThrow();
    if(bounds.maxCalls!==undefined&&BigInt(counts.calls)>=BigInt(bounds.maxCalls)||bounds.concurrentCalls!==undefined&&BigInt(active.calls)>=BigInt(bounds.concurrentCalls)||bounds.maxTokens!==undefined&&BigInt(counts.tokens)+BigInt(input.estimate.value)+BigInt(input.maxOutputTokens)>BigInt(bounds.maxTokens))throw problem('models.call_budget','budget','模型调用数、并发或Token额度不足');
    for(const period of ['total','daily'] as const){
     const limit=period==='total'?bounds.totalAmount:bounds.dailyAmount;if(!limit||!reserved)continue;
     let query=base();
     if(period==='daily'){const start=Temporal.Instant.from(at).toZonedDateTimeISO(bounds.timeZone??'UTC').startOfDay().toInstant().toString({fractionalSecondDigits:6});query=query.where('recordedAt','>=',start);}
     else if(bounds.window){const start=Temporal.Instant.from(at).subtract({milliseconds:bounds.window.durationMs}).toString({fractionalSecondDigits:6});query=query.where('recordedAt','>=',start);}
     const spent=await query.select([sql<string>`coalesce(sum(coalesce(actual_cost,reserved_cost)) filter(where currency=${limit.currency}),0)`.as('amount'),sql<string>`count(*) filter(where actual_cost is null and (reserved_cost is null or currency is null))`.as('unknown')]).executeTakeFirstOrThrow();
     if(BigInt(spent.unknown)>0n||new Exact(spent.amount).plus(reserved.amount).gt(limit.amount))throw problem('models.amount_budget','budget','模型金额额度不足或已有费用待核实');
    }
   }
   const callIdentity={...input.identity,invocationId:existing?.id??input.identity.invocationId??uuid()};
   await options.budgetScopes.admit?.(ctx,{identity:callIdentity,snapshot:input.snapshot,reservedCost:reserved,estimate:input.estimate,maxOutputTokens:input.maxOutputTokens},db);
   if(existing){const updated=await db.updateTable('modelInvocations').set({budgetSnapshot:json({constraints}),budgetKeys:json(budgetKeys),reservedCost:reserved?.amount??null,currency:reserved?.currency??null}).where('id','=',existing.id).returningAll().executeTakeFirstOrThrow();return view(updated);}
   const row=await db.insertInto('modelInvocations').values({id:callIdentity.invocationId,logicalOperationId:callIdentity.logicalOperationId,physicalOrdinal:callIdentity.physicalOrdinal,attemptNo:callIdentity.attemptNo,workflowId:callIdentity.workflowId??null,stepNo:callIdentity.stepNo??null,batchNo:callIdentity.batchNo??null,subjectId:ctx.subjectId??null,consumer:json(ctx.consumer),purpose:ctx.purpose,audience:json(ctx.audience),authorityRef:ctx.actor.authorityRef,traceId:ctx.trace.traceId,roleId:input.snapshot.roleId,modelKind:input.snapshot.policy.kind,status:'prepared',requestDigest,completionDigest:null,configSnapshot:json(input.snapshot),preparation:json(input.preparation??null),resourceId:input.snapshot.actualResource.id,resourceRevision:input.snapshot.actualResource.revision,actualProvider:input.snapshot.actualProvider,actualModelId:input.snapshot.actualModelId,inputSources:json(input.inputRefs),semanticInputRefs:json(input.semanticInputRefs??[]),outputRefs:json([]),observedCapture:json(input.observed??null),formedCapture:json(null),tokenEstimate:json(input.estimate),maxOutputTokens:input.maxOutputTokens,usage:json(null),rawUsage:json(null),budgetSnapshot:json({constraints}),budgetKeys:json(budgetKeys),reservedCost:reserved?.amount??null,actualCost:null,currency:reserved?.currency??null,actualTokens:null,providerRequestId:null,problem:json(null),recordedAt:at,completedAt:null,nativeIdentity:json(input.native??null),adoptedAt:null}).returningAll().executeTakeFirstOrThrow();
   return view(row);
  });
 }
 async function dispatch(ctx:CallContext,id:string,observed:TimeCaptureReceipt){
  await options.beforeDispatch?.();const current=await own(ctx,id);await options.data.validateSources(ctx,current.inputSources);
  const capture=parse(TimeCaptureReceiptSchema,observed);
  await options.authorize(ctx,'models.invoke',modelRef(current.configSnapshot));
  const row=await options.db.updateTable('modelInvocations').set({status:'dispatched',observedCapture:json(capture)}).where('id','=',id).where('status','=','prepared').returningAll().executeTakeFirst();
  if(!row)throw problem('models.dispatch_unknown','effect_unknown','该调用已有派发事实，请核实原结果',{operationId:ctx.trace.operationId,retry:{mode:'reconcile'}});
  return view(row);
 }
 async function terminal(ctx:CallContext,id:string,input:{status:'completed'|'not_sent'|'unknown';outputRefs?:Ref[];metadata?:TerminalMetadata;realAt?:string}){
  const current=await fact(id),usage=input.metadata?.usage?parse(NormalizedUsageSchema,input.metadata.usage):undefined;
  const actual=input.status==='not_sent'?null:modelCost(current.configSnapshot.pricing,usage);
  const request={status:input.status,usage:usage??null,providerRequestId:input.metadata?.providerRequestId??null};const completionDigest=digest(request);
  return options.db.transaction().execute(async db=>{
   const row=await db.selectFrom('modelInvocations').selectAll().where('id','=',id).forUpdate().executeTakeFirstOrThrow();
   if(row.completionDigest&&row.status!=='unknown'){if(row.completionDigest!==completionDigest)throw problem('models.completion_conflict','conflict','模型调用已有不同完成结果');return view(row);}
   if(input.status==='completed'&&!['dispatched','unknown'].includes(row.status))throw problem('models.completion_not_dispatched','conflict','没有实际派发记录的调用不能完成');
   const updated=await db.updateTable('modelInvocations').set({status:input.status,completionDigest,outputRefs:json(input.outputRefs??[]),usage:json(usage??null),rawUsage:json(input.metadata?.rawUsage??null),providerRequestId:input.metadata?.providerRequestId??null,actualCost:input.status==='not_sent'?'0':actual?.amount??null,currency:actual?.currency??row.currency,reservedCost:input.status==='not_sent'||actual?'0':row.reservedCost,actualTokens:usage?.inputTokens!==undefined&&usage?.outputTokens!==undefined?(BigInt(usage.inputTokens)+BigInt(usage.outputTokens)).toString():null,problem:json(input.metadata?.problem??null),completedAt:input.status==='unknown'?null:input.realAt??readReal().at}).where('id','=',id).returningAll().executeTakeFirstOrThrow();return view(updated);
  });
 }
 async function adopt(ctx:CallContext,id:string,formed?:TimeCaptureReceipt,recordingContext:CallContext=ctx){
  let row=await fact(id);if(row.status!=='completed'||row.adoptedAt)return view(row);
  if(!row.observedCapture)throw problem('models.observed_missing','conflict','实际调用缺少原观察时间回执');
  const capture=row.formedCapture??parse(TimeCaptureReceiptSchema,formed??await options.clock.captureTerminal(recordingContext,{operationId:`${row.logicalOperationId}/step${row.physicalOrdinal}/formed`,originRef:ref(row.id,row.subjectId),consumer:row.consumer,observed:row.observedCapture,realAt:parseInstant(row.completedAt!)}));
  if(!row.formedCapture)row=await options.db.updateTable('modelInvocations').set({formedCapture:json(capture)}).where('id','=',id).returningAll().executeTakeFirstOrThrow();
  if(!row.outputRefs.length)return view(row);
  const call=view(row);await options.data.adopt(ctx,{call,inputRefs:row.semanticInputRefs,outputRefs:row.outputRefs});await options.budgetScopes.settle?.(ctx,call);
  const adopted=await options.db.updateTable('modelInvocations').set({adoptedAt:readReal().at,...(row.problem?.code==='models.adoption_pending'?{problem:json(null)}:{})}).where('id','=',id).returningAll().executeTakeFirstOrThrow();return view(adopted);
 }
 return {prepare,dispatch,terminal,adopt,own,fact,view,
  async deferAdoption(id:string){const row=await options.db.updateTable('modelInvocations').set({problem:json(problem('models.adoption_pending','unavailable','实际调用与费用已保存，形成时间或成果仍待接纳',{operationId:id,retry:{mode:'after_change'}}).problem)}).where('id','=',id).where('status','=','completed').returningAll().executeTakeFirstOrThrow();return view(row);},
  async recheck(ctx:CallContext,id:string,constraints:BudgetConstraint[]){const row=await own(ctx,id);return prepare(ctx,{identity:identity(row),snapshot:row.configSnapshot,constraints,inputRefs:row.inputSources,estimate:row.tokenEstimate,maxOutputTokens:row.maxOutputTokens,observed:row.observedCapture??undefined,semanticInputRefs:row.semanticInputRefs,native:row.nativeIdentity??undefined,preparation:row.preparation??undefined});},
  async attachOutputs(ctx:CallContext,id:string,outputRefs:Ref[]){const row=await fact(id);if(row.status!=='completed')throw problem('models.output_not_completed','conflict','仅完成调用可以接纳输出');if(row.outputRefs.length&&digest(row.outputRefs)!==digest(outputRefs))throw problem('models.output_conflict','conflict','已保存不同模型输出');const updated=await options.db.updateTable('modelInvocations').set({outputRefs:json(outputRefs)}).where('id','=',id).returningAll().executeTakeFirstOrThrow();return view(updated);},
  async read(ctx:CallContext,input:{invocationId:string}){return view(await selected(ctx,input.invocationId));},
  async list(ctx:CallContext,input:unknown={},operation:'models.invocations.read'|'costs.read'='models.invocations.read'){
   const filter=parse(ModelInvocationFilterSchema,input);await options.authorize(ctx,operation);
   const subjectId=filter.subjectId??ctx.subjectId;if(!subjectId)await options.authorize(ctx,'installation.manage');
   const scope=digest({subjectId:subjectId??null,roleId:filter.roleId??null,status:filter.status??null,logicalOperationId:filter.logicalOperationId??null});let before:{at:string;id:string}|undefined;
   if(filter.cursor){try{const value=JSON.parse(Buffer.from(filter.cursor,'base64url').toString());if(value.scope!==scope||typeof value.id!=='string'||typeof value.at!=='string')throw Error();before={at:parseInstant(value.at),id:value.id};}catch{throw problem('models.invalid_cursor','validation','调用游标与查询范围不匹配');}}
   let query=options.db.selectFrom('modelInvocations').selectAll();if(subjectId)query=query.where('subjectId','=',subjectId);if(filter.roleId)query=query.where('roleId','=',filter.roleId);if(filter.logicalOperationId)query=query.where('logicalOperationId','=',filter.logicalOperationId);if(filter.status)query=query.where('status','=',filter.status);if(before)query=query.where(sql<boolean>`(recorded_at,id)<(${before.at}::timestamptz,${before.id}::uuid)`);
   const limit=filter.limit??50,rows=await query.orderBy('recordedAt','desc').orderBy('id','desc').limit(limit+1).execute();const items:ModelCallView[]=[];for(const row of rows.slice(0,limit)){await options.authorize(ctx,operation,ref(row.id,row.subjectId));items.push(view(row));}
   const last=items.at(-1);return immutable({items,...(rows.length>limit&&last?{nextCursor:Buffer.from(JSON.stringify({scope,at:last.recordedAt,id:last.identity.invocationId})).toString('base64url')}:{})});
  },
  async reconcile(ctx:CallContext,raw:unknown){
   const input=parse(ModelReconcileSchema,raw),row=await fact(input.invocationId);
   await options.authorize(ctx,'models.reconcile',ref(row.id,row.subjectId));
   if(row.status==='prepared'&&input.status!=='not_sent')throw problem('models.not_reconcilable','conflict','没有派发事实的调用只能确认未发送');
   if(!['prepared','dispatched','unknown','completed','not_sent'].includes(row.status))throw problem('models.not_reconcilable','conflict','调用状态不支持核实');
   if(['completed','not_sent'].includes(row.status)){
    if(row.status!==input.status||input.usage&&digest(input.usage)!==digest(row.usage)||input.providerRequestId!==undefined&&input.providerRequestId!==row.providerRequestId||input.outputRefs!==undefined&&digest(input.outputRefs)!==digest(row.outputRefs))throw problem('models.completion_conflict','conflict','模型调用已有不同完成结果');
    return row.status==='completed'?adopt(ctx,row.id,input.formed?await options.clock.validateCapture(ctx,input.formed):undefined):view(row);
   }
   const result=await terminal(ctx,input.invocationId,{status:input.status,outputRefs:input.outputRefs,metadata:{usage:input.usage,providerRequestId:input.providerRequestId}});
   return result.status==='completed'?adopt(ctx,input.invocationId,input.formed?await options.clock.validateCapture(ctx,input.formed):undefined):result;
  },
 };
}
export type InvocationOwner=ReturnType<typeof createInvocationOwner>;
