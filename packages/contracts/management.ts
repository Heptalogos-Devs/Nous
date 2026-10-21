import { z } from 'zod';
export const GrowthSettingsSchema=z.strictObject({enabled:z.boolean(),batchSize:z.number().int().min(1).max(24),intervalMinutes:z.number().int().positive().safe(),journalEnabled:z.boolean(),journalTimeZone:z.string().refine(value=>{try{new Intl.DateTimeFormat('en',{timeZone:value});return true;}catch{return false;}},'时区无效')});
export type GrowthSettings=z.infer<typeof GrowthSettingsSchema>;
export const RetrievalSettingsSchema=z.strictObject({limit:z.number().int().min(1).max(100),exploration:z.number().int().min(0).max(3)});
export type RetrievalSettings=z.infer<typeof RetrievalSettingsSchema>;

for(const [id,schema] of Object.entries({GrowthSettingsSchema,RetrievalSettingsSchema}))z.globalRegistry.add(schema,{id:id.replace(/Schema$/,'')});
