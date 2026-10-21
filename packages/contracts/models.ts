// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import {z} from 'zod';
import {ConsumerSchema,InstantSchema,JsonValueSchema,MoneySchema,RefSchema,SubjectIdSchema,ProblemSchema} from './common.js';
import {RevisionRefSchema} from './settings.js';
import {PromptBindingSchema,PromptSnapshotRefSchema} from './prompts.js';
import {TimeCaptureReceiptSchema,CapturedTimeSchema} from './time.js';

const id=z.string().min(1).max(256);
const positive=z.number().int().positive().safe();
const nonnegative=z.number().int().nonnegative().safe();
const object=z.record(z.string(),JsonValueSchema);
export const ModelKindSchema=z.enum(['language','embedding','reranking','transcription','speech']);
export const ResourceRefSchema=z.strictObject({id,revision:positive});
export const ProviderOptionsSchema=z.record(z.string(),object);
export const ReasoningSchema=z.enum(['provider-default','none','minimal','low','medium','high','xhigh','max']);
export const ModelSelectionSchema=z.enum(['ordered','balance','random']);
export const ProviderDefinitionSchema=z.strictObject({id,adapter:id,baseURL:z.url().optional(),credentialRef:z.string().min(1).optional(),networkProfileId:id,options:object,enabled:z.boolean()});
export const ModelResourceSchema=z.strictObject({id,label:z.string().min(1),providerId:id,modelId:z.string().min(1),kind:ModelKindSchema,enabled:z.boolean(),capabilities:z.array(id),contextLimit:positive.optional(),outputLimit:positive.optional(),providerOptions:ProviderOptionsSchema,pricingRef:id.optional()});
export const NativeResourceRegistrationSchema=z.strictObject({id,runtimeId:id,nativeResourceId:z.string().min(1),enabled:z.boolean(),pricingRef:id.optional()});
export const NativeModelBindingSchema=z.strictObject({id,runtimeId:id,nativeResourceId:z.string().min(1),configRevision:z.string().min(1),actualProvider:z.string().min(1),actualModelId:z.string().min(1),kind:ModelKindSchema,capabilities:z.array(id),contextLimit:positive.optional(),outputLimit:positive.optional()});
export const ModelRegistrationSchema=z.discriminatedUnion('managedBy',[
 z.strictObject({managedBy:z.literal('sdk'),definition:ModelResourceSchema}),
 z.strictObject({managedBy:z.literal('native'),definition:NativeResourceRegistrationSchema}),
]);
export const ModelDefinitionSchema=z.discriminatedUnion('managedBy',[
 z.strictObject({managedBy:z.literal('sdk'),definition:ModelResourceSchema}),
 z.strictObject({managedBy:z.literal('native'),id,binding:NativeModelBindingSchema,enabled:z.boolean(),pricingRef:id.optional()}),
]);
export const LanguageSettingsSchema=z.strictObject({maxOutputTokens:positive,temperature:z.number().finite().optional(),reasoning:ReasoningSchema.optional(),providerOptions:ProviderOptionsSchema});
const role={enabled:z.boolean(),resources:z.array(ResourceRefSchema),selection:ModelSelectionSchema,timeoutMs:positive,budgetPolicyRef:id.optional(),prompt:PromptBindingSchema.optional()};
export const RolePolicySchema=z.discriminatedUnion('kind',[
 z.strictObject({...role,kind:z.literal('language'),inference:LanguageSettingsSchema}),
 z.strictObject({...role,kind:z.literal('embedding'),inference:z.strictObject({maxParallelCalls:positive,providerOptions:ProviderOptionsSchema})}),
 z.strictObject({...role,kind:z.literal('reranking'),inference:z.strictObject({topN:positive.optional(),providerOptions:ProviderOptionsSchema})}),
 z.strictObject({...role,kind:z.literal('transcription'),inference:z.strictObject({providerOptions:ProviderOptionsSchema})}),
 z.strictObject({...role,kind:z.literal('speech'),inference:z.strictObject({voice:z.string().optional(),outputFormat:z.string().optional(),speed:z.number().positive().finite().optional(),language:z.string().optional(),providerOptions:ProviderOptionsSchema})}),
]).superRefine((value,ctx)=>{if(value.enabled&&!value.resources.length)ctx.addIssue({code:'custom',path:['resources'],message:'启用的职责需要至少一个模型资源'});});
export const UsageQuantitySchema=z.strictObject({unit:z.string().min(1),quantity:z.string().regex(/^(?:0|[1-9]\d*)(?:\.\d+)?$/),source:z.enum(['provider','measured','estimated'])});
export const NormalizedUsageSchema=z.strictObject({inputTokens:nonnegative.optional(),outputTokens:nonnegative.optional(),cacheReadTokens:nonnegative.optional(),cacheWriteTokens:nonnegative.optional(),reasoningTokens:nonnegative.optional(),units:z.array(UsageQuantitySchema).optional()}).superRefine((value,ctx)=>{if(value.inputTokens!==undefined&&BigInt(value.cacheReadTokens??0)+BigInt(value.cacheWriteTokens??0)>BigInt(value.inputTokens))ctx.addIssue({code:'custom',message:'缓存输入分项不能超过总输入'});if(value.outputTokens!==undefined&&(value.reasoningTokens??0)>value.outputTokens)ctx.addIssue({code:'custom',message:'推理分项不能超过总输出'});});
const nonnegativeMoney=MoneySchema.refine(value=>!value.amount.startsWith('-'),'预算金额必须非负');
export const ModelBudgetSchema=z.strictObject({maxCalls:positive.optional(),maxTokens:positive.optional(),concurrentCalls:positive.optional(),totalAmount:nonnegativeMoney.nullable().optional(),dailyAmount:nonnegativeMoney.nullable().optional(),perCallAmount:nonnegativeMoney.nullable().optional(),window:z.strictObject({kind:z.literal('rolling'),durationMs:positive}).optional(),timeZone:z.string().min(1).optional()});
export const PriceRateSchema=z.strictObject({unit:z.string().min(1),amount:nonnegativeMoney,per:z.string().regex(/^(?:[1-9]\d*)(?:\.\d+)?$/)});
export const PriceDefinitionSchema=z.strictObject({id,rates:z.array(PriceRateSchema).min(1)}).superRefine((value,ctx)=>{if(new Set(value.rates.map(rate=>rate.amount.currency)).size!==1)ctx.addIssue({code:'custom',message:'一份模型价目必须使用同一币种'});if(new Set(value.rates.map(rate=>rate.unit)).size!==value.rates.length)ctx.addIssue({code:'custom',message:'同一价目单位不能重复'});if(value.rates.some(rate=>rate.unit==='input_tokens')&&value.rates.some(rate=>['uncached_input_tokens','cache_read_tokens','cache_write_tokens'].includes(rate.unit)))ctx.addIssue({code:'custom',message:'总输入价格不能与输入缓存分项重复计价'});});
export const PriceBookSchema=PriceDefinitionSchema.safeExtend({revision:positive});
export const BudgetAllocationUseSchema=z.strictObject({allocationRef:RefSchema,childOperationId:z.string().min(1)});
export const BudgetAllocationSnapshotSchema=z.strictObject({allocationRef:RefSchema,childOperationId:z.string().min(1),receiptDigest:z.string().min(1),taskRef:RefSchema,budgetOwnerRef:RefSchema,issuer:z.strictObject({product:z.literal('hept'),installationId:z.string().min(1),authorityRef:z.string().min(1)}),target:z.strictObject({product:z.enum(['nous','hept']),installationId:z.string().min(1),subjectId:SubjectIdSchema,consumer:ConsumerSchema}),limits:z.strictObject({totalAmount:nonnegativeMoney,perCallAmount:nonnegativeMoney.optional(),maxCalls:positive.optional(),maxTokens:positive.optional()}),expiresAt:z.strictObject({domainId:z.literal('real'),at:InstantSchema})}).superRefine((value,ctx)=>{if(value.limits.perCallAmount&&value.limits.perCallAmount.currency!==value.limits.totalAmount.currency)ctx.addIssue({code:'custom',message:'子额度金额必须使用相同币种'});});
export const InvocationSnapshotSchema=z.strictObject({config:z.strictObject({revisions:z.array(RevisionRefSchema),definitionRevisions:z.record(z.string(),z.string()),digest:z.string().min(1)}),roleId:id,requestedResource:ResourceRefSchema,actualResource:ResourceRefSchema,actualProvider:z.string().min(1),actualModelId:z.string().min(1),definition:ModelDefinitionSchema,provider:ProviderDefinitionSchema.optional(),policy:RolePolicySchema,inference:object,prompt:PromptSnapshotRefSchema.optional(),credential:z.strictObject({ref:z.string().min(1),revision:positive,generation:positive}).optional(),network:z.strictObject({profileId:id,revisions:z.array(RevisionRefSchema)}).optional(),pricing:PriceBookSchema.optional(),budgetAllocation:BudgetAllocationSnapshotSchema.optional()});
export const ModelInvocationIdentitySchema=z.strictObject({invocationId:z.uuidv7(),logicalOperationId:z.string().min(1),physicalOrdinal:nonnegative,stepNo:nonnegative.optional(),batchNo:nonnegative.optional(),attemptNo:positive,workflowId:z.string().min(1).optional()});
export const NativeOriginSchema=z.strictObject({type:z.literal('native'),runtimeId:z.string().min(1),sessionId:z.string().min(1),sessionKey:z.string().min(1),runId:z.string().min(1),entryId:z.string().optional(),callId:z.string().min(1)});
export const TokenEstimateSchema=z.strictObject({value:nonnegative,method:z.string().min(1)});
export const ModelPreparationSchema=z.strictObject({referenceTime:CapturedTimeSchema,inputDigest:z.string().regex(/^[a-f0-9]{64}$/).optional(),contextRef:RefSchema.extend({kind:z.literal('ContextView')}).optional()});
export const NativeInvocationPrepareSchema=z.strictObject({invocationId:z.uuidv7(),roleId:id,modelResourceId:id,actualProvider:z.string().min(1),actualModelId:z.string().min(1),native:NativeOriginSchema,inputTokensEstimate:TokenEstimateSchema,maxOutputTokens:positive,inputRefs:z.array(RefSchema),preparation:ModelPreparationSchema.optional(),budgetAllocation:BudgetAllocationUseSchema.optional()});
export const NativeInvocationCompletionSchema=z.strictObject({invocationId:z.uuidv7(),status:z.enum(['completed','not_sent','unknown']),usage:NormalizedUsageSchema.optional(),rawUsage:JsonValueSchema.optional(),providerRequestId:z.string().optional(),outputRef:RefSchema.optional(),formed:TimeCaptureReceiptSchema.optional()}).superRefine((value,ctx)=>{if(value.status!=='completed'&&(value.outputRef||value.formed))ctx.addIssue({code:'custom',message:'未发送或状态未知的调用不能提交已形成输出'});});
export const ModelCallViewSchema=z.strictObject({identity:ModelInvocationIdentitySchema,authorityRef:z.string().min(1),native:NativeOriginSchema.optional(),purpose:z.string(),audience:z.array(z.string()),traceId:z.string().regex(/^[a-f0-9]{32}$/),subjectId:SubjectIdSchema.optional(),consumer:ConsumerSchema,roleId:id,kind:ModelKindSchema,status:z.enum(['prepared','dispatched','completed','not_sent','unknown']),snapshot:InvocationSnapshotSchema,preparation:ModelPreparationSchema.optional(),inputRefs:z.array(RefSchema),outputRefs:z.array(RefSchema),observed:TimeCaptureReceiptSchema.optional(),formed:TimeCaptureReceiptSchema.optional(),usage:NormalizedUsageSchema.optional(),providerRequestId:z.string().optional(),reservedCost:MoneySchema.nullable(),actualCost:MoneySchema.nullable(),problem:ProblemSchema.optional(),recordedAt:InstantSchema,completedAt:InstantSchema.optional()});
export const ModelInvocationFilterSchema=z.strictObject({logicalOperationId:z.string().min(1).max(1024).optional(),subjectId:SubjectIdSchema.optional(),roleId:id.optional(),status:z.enum(['prepared','dispatched','completed','not_sent','unknown']).optional(),cursor:z.string().max(8192).optional(),limit:z.number().int().min(1).max(200).optional()});
export const ModelInvocationPageSchema=z.strictObject({items:z.array(ModelCallViewSchema),nextCursor:z.string().optional()});
export const ModelReconcileSchema=z.strictObject({invocationId:z.uuidv7(),status:z.enum(['completed','not_sent']),usage:NormalizedUsageSchema.optional(),outputRefs:z.array(RefSchema).optional(),providerRequestId:z.string().optional(),formed:TimeCaptureReceiptSchema.optional()});
export type ModelKind=z.infer<typeof ModelKindSchema>;
export type Reasoning=z.infer<typeof ReasoningSchema>;
export type ResourceRef=z.infer<typeof ResourceRefSchema>;
export type ProviderDefinition=z.infer<typeof ProviderDefinitionSchema>;
export type ModelResource=z.infer<typeof ModelResourceSchema>;
export type ModelRegistration=z.infer<typeof ModelRegistrationSchema>;
export type NativeModelBinding=z.infer<typeof NativeModelBindingSchema>;
export type ModelDefinition=z.infer<typeof ModelDefinitionSchema>;
export type RolePolicy=z.infer<typeof RolePolicySchema>;
export type NormalizedUsage=z.infer<typeof NormalizedUsageSchema>;
export type ModelBudget=z.infer<typeof ModelBudgetSchema>;
export type PriceBook=z.infer<typeof PriceBookSchema>;
export type BudgetAllocationUse=z.infer<typeof BudgetAllocationUseSchema>;
export type BudgetAllocationSnapshot=z.infer<typeof BudgetAllocationSnapshotSchema>;
export type InvocationSnapshot=z.infer<typeof InvocationSnapshotSchema>;
export type ModelInvocationIdentity=z.infer<typeof ModelInvocationIdentitySchema>;
export type ModelCallView=z.infer<typeof ModelCallViewSchema>;
export type ModelReconcile=z.infer<typeof ModelReconcileSchema>;
export type NativeInvocationPrepare=z.infer<typeof NativeInvocationPrepareSchema>;
export type NativeInvocationCompletion=z.infer<typeof NativeInvocationCompletionSchema>;
export type TokenEstimate=z.infer<typeof TokenEstimateSchema>;
export type ModelPreparation=z.infer<typeof ModelPreparationSchema>;
export const providerDefaults=Object.freeze({enabled:true,options:{}});
export const modelResourceDefaults=Object.freeze({enabled:false,capabilities:[] as string[],providerOptions:{}});
export const languageSettingsDefaults=Object.freeze({maxOutputTokens:2048,providerOptions:{}});
for(const[id,schema]of Object.entries({ModelKindSchema,ResourceRefSchema,ProviderOptionsSchema,ReasoningSchema,ModelSelectionSchema,ProviderDefinitionSchema,ModelResourceSchema,ModelRegistrationSchema,NativeModelBindingSchema,ModelDefinitionSchema,LanguageSettingsSchema,RolePolicySchema,UsageQuantitySchema,NormalizedUsageSchema,ModelBudgetSchema,PriceRateSchema,PriceDefinitionSchema,PriceBookSchema,BudgetAllocationUseSchema,BudgetAllocationSnapshotSchema,InvocationSnapshotSchema,ModelInvocationIdentitySchema,NativeOriginSchema,TokenEstimateSchema,ModelPreparationSchema,NativeInvocationPrepareSchema,NativeInvocationCompletionSchema,ModelCallViewSchema,ModelInvocationFilterSchema,ModelInvocationPageSchema,ModelReconcileSchema}))z.globalRegistry.add(schema,{id:id.replace(/Schema$/,'')});

export const ModelTestInputSchema=z.strictObject({roleId:z.string().min(1),prompt:z.string().min(1).max(262144),promptParams:JsonValueSchema,resource:ResourceRefSchema.optional(),maxOutputTokens:z.number().int().positive().optional()});
export type ModelTestInput=z.infer<typeof ModelTestInputSchema>;
z.globalRegistry.add(ModelTestInputSchema,{id:'ModelTestBody'});

const dataText=z.string().min(1),dataOrdinal=z.number().int().nonnegative().safe(),dataRevision=z.number().int().positive().safe(),dataAmount=z.string().regex(/^(?:0|[1-9]\d*)(?:\.\d+)?$/);
export const ModelDataRecordSchema=z.strictObject({
 id:z.uuidv7(),logicalOperationId:dataText,physicalOrdinal:dataOrdinal,attemptNo:dataRevision,workflowId:dataText.nullable(),stepNo:dataOrdinal.nullable(),batchNo:dataOrdinal.nullable(),
 subjectId:SubjectIdSchema.nullable(),consumer:ConsumerSchema,purpose:z.string(),audience:z.array(z.string()),authorityRef:dataText,traceId:dataText,roleId:dataText,modelKind:ModelKindSchema,
 status:z.enum(['prepared','dispatched','completed','not_sent','unknown']),requestDigest:dataText,completionDigest:dataText.nullable(),configSnapshot:InvocationSnapshotSchema,
 resourceId:dataText,resourceRevision:dataRevision,actualProvider:dataText,actualModelId:dataText,inputSources:z.array(RefSchema),semanticInputRefs:z.array(RefSchema),outputRefs:z.array(RefSchema),
 observedCapture:TimeCaptureReceiptSchema.nullable(),formedCapture:TimeCaptureReceiptSchema.nullable(),preparation:ModelPreparationSchema.nullable(),
 tokenEstimate:TokenEstimateSchema,maxOutputTokens:dataOrdinal,usage:NormalizedUsageSchema.nullable(),rawUsage:JsonValueSchema,
 budgetSnapshot:z.strictObject({constraints:z.array(z.strictObject({key:dataText,budget:ModelBudgetSchema}))}),budgetKeys:z.array(dataText),
 reservedCost:dataAmount.nullable(),actualCost:dataAmount.nullable(),currency:dataText.nullable(),actualTokens:z.string().regex(/^\d+$/).nullable(),providerRequestId:dataText.nullable(),problem:ProblemSchema.nullable(),recordedAt:InstantSchema,completedAt:InstantSchema.nullable(),nativeIdentity:JsonValueSchema,adoptedAt:InstantSchema.nullable(),
}).superRefine((call,ctx)=>{
 const snapshot=call.configSnapshot;
 if(call.roleId!==snapshot.roleId||call.modelKind!==snapshot.policy.kind||call.resourceId!==snapshot.actualResource.id||call.resourceRevision!==snapshot.actualResource.revision||call.actualProvider!==snapshot.actualProvider||call.actualModelId!==snapshot.actualModelId)ctx.addIssue({code:'custom',message:'调用事实与冻结资源身份不符'});
 if(call.status==='completed'&&(!call.completedAt||!call.completionDigest))ctx.addIssue({code:'custom',message:'完成事实缺少实际完成身份'});
});
export const ModelDataBundleSchema=z.strictObject({formatVersion:z.literal(1),sourceProduct:z.enum(['nous','hept']),sourceInstallationId:dataText,subjectId:SubjectIdSchema.nullable(),calls:z.array(ModelDataRecordSchema)});
export type ModelDataBundle=z.infer<typeof ModelDataBundleSchema>;

z.globalRegistry.add(ModelDataRecordSchema,{id:'ModelDataRecord'});
z.globalRegistry.add(ModelDataBundleSchema,{id:'ModelDataBundle'});
