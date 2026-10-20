import { z } from 'zod';
import { RefSchema } from './common.js';
export const RoleProfileSchema=z.object({modelResourceId:z.string().min(1).meta({title:'模型资源'}),temperature:z.number().min(0).max(2).default(0.7).meta({title:'温度'}),maxOutputTokens:z.number().int().positive().default(2048).meta({title:'最大输出 token'}),reasoning:z.string().optional()});
export const BudgetSchema=z.object({
  dailyAmount:z.number().nonnegative().nullable().default(null).meta({title:'每日金额上限'}),
  dailyCalls:z.number().int().positive().default(100).meta({title:'每日调用上限'}),
  perCallAmount:z.number().nonnegative().nullable().default(null).meta({title:'单次金额上限'}),
  dailyInputTokens:z.number().int().positive().nullable().default(null).meta({title:'每日输入 token 上限'}),
  dailyOutputTokens:z.number().int().positive().nullable().default(null).meta({title:'每日输出 token 上限'}),
  concurrentCalls:z.number().int().positive().default(4).meta({title:'并发调用上限'}),
}).meta({title:'模型预算','x-scopes':['installation','subject','role'],'x-effect':'next_call'});
export const GrowthSettingsSchema=z.object({enabled:z.boolean().default(true),batchSize:z.number().int().min(1).max(24).default(24),intervalMinutes:z.number().int().positive().default(30),journalEnabled:z.boolean().default(true),journalTimeZone:z.string().refine(value=>{try{new Intl.DateTimeFormat('en',{timeZone:value});return true;}catch{return false;}},'时区无效').default('UTC')}).meta({title:'后台成长','x-scopes':['installation','subject'],'x-effect':'next_call'});
export const RetrievalSettingsSchema=z.object({limit:z.number().int().min(1).max(100).default(12),exploration:z.number().int().min(0).max(3).default(2)}).meta({title:'认知检索','x-scopes':['installation','subject'],'x-effect':'next_call'});
export const SettingsSchema=z.object({
  roles:z.record(z.string(),RoleProfileSchema).default({}).meta({title:'角色模型','x-scopes':['installation','subject','role']}),
  budget:BudgetSchema.prefault({}),
  growth:GrowthSettingsSchema.prefault({}),
  retrieval:RetrievalSettingsSchema.prefault({}),
});
export type Settings=z.infer<typeof SettingsSchema>;
export const SettingsPatchSchema=z.object({roles:z.record(z.string(),RoleProfileSchema.partial()).optional(),budget:BudgetSchema.partial().optional(),growth:GrowthSettingsSchema.partial().optional(),retrieval:RetrievalSettingsSchema.partial().optional()}).strict();
export const SettingsScopeSchema=z.object({subjectId:z.uuidv7().optional(),scope:z.enum(['installation','subject','role']).default('subject'),role:z.string().optional()});
export const SettingsUpdateSchema=SettingsScopeSchema.extend({values:SettingsPatchSchema,expectedRevision:z.number().int().nonnegative().optional(),reason:z.string().optional()});
export const SettingsResetSchema=SettingsScopeSchema.extend({paths:z.array(z.array(z.string().min(1)).min(1)).optional(),expectedRevision:z.number().int().nonnegative().optional(),reason:z.string().optional()});
export const SettingsRestoreSchema=SettingsScopeSchema.extend({revision:z.number().int().positive(),expectedRevision:z.number().int().nonnegative().optional(),reason:z.string().optional()});
export const ModelResourceSchema=z.object({id:z.string().min(1),provider:z.literal('openai-compatible'),modelId:z.string(),baseURL:z.url(),credentialRef:z.string(),capabilities:z.array(z.enum(['text','vision','tools','structured','embedding'])),contextLimit:z.number().int().positive(),outputLimit:z.number().int().positive(),pricing:z.object({inputPerMillion:z.number().nonnegative(),outputPerMillion:z.number().nonnegative(),currency:z.string().default('USD')}).nullable().default(null),providerOptions:z.record(z.string(),z.unknown()).default({})});
export type ModelResource=z.infer<typeof ModelResourceSchema>;

// Named public schemas are shared by OpenAPI references.
for (const [id, schema] of Object.entries({RoleProfileSchema,BudgetSchema,SettingsSchema,SettingsPatchSchema,SettingsScopeSchema,SettingsUpdateSchema,SettingsResetSchema,SettingsRestoreSchema,ModelResourceSchema})) z.globalRegistry.add(schema, { id: id.replace(/Schema$/, '') });

export const InvocationPrepareSchema=z.object({
  invocationId:z.string().min(1),role:z.string(),modelResourceId:z.string(),inputTokensEstimate:z.number().int().nonnegative(),
  maxOutputTokens:z.number().int().positive(),inputRefs:z.array(RefSchema).default([]),nativeRunRef:RefSchema.optional(),
});
export const InvocationCompletionSchema=z.object({
  status:z.enum(['completed','not_sent','unknown']),inputTokens:z.number().int().nonnegative().optional(),outputTokens:z.number().int().nonnegative().optional(),
  cacheReadTokens:z.number().int().nonnegative().optional(),cacheWriteTokens:z.number().int().nonnegative().optional(),
  providerRequestId:z.string().optional(),outputRef:RefSchema.optional(),rawUsage:z.record(z.string(),z.unknown()).optional(),
});

for(const [id,schema] of Object.entries({InvocationPrepareSchema,InvocationCompletionSchema}))z.globalRegistry.add(schema,{id:id.replace(/Schema$/,'')});
