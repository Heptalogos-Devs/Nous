import { z } from 'zod';
import { PromptLocaleSchema } from '@nous-wave/contracts/prompts';
import type { FieldMetadata, SettingsModule } from '../settings/types.js';
export const PromptsSettingsSchema=z.strictObject({
  locale:PromptLocaleSchema,watch:z.strictObject({enabled:z.boolean()}),
  limits:z.strictObject({sourceBytes:z.number().int().min(1).max(131072),closureBytes:z.number().int().min(1).max(524288),outputBytes:z.number().int().min(1).max(262144),closureCount:z.number().int().min(1).max(32),renderMs:z.number().int().min(1).max(100),memoryBytes:z.number().int().min(1024).max(1048576)}),
});
export const promptsDefaults={locale:'zh-CN',watch:{enabled:false},limits:{sourceBytes:131072,closureBytes:524288,outputBytes:262144,closureCount:32,renderMs:100,memoryBytes:1048576}};
export const promptsDefinition:SettingsModule<z.infer<typeof PromptsSettingsSchema>>={id:'platform.prompts',owner:'platform.prompts',definitionRevision:'prompts-1',schema:PromptsSettingsSchema,defaults:promptsDefaults,fields:Object.fromEntries([
  ['/locale',{labelKey:'prompts.locale',descriptionKey:'prompts.locale.description',scopes:['installation','subject'],visibility:'standard',effect:'next_call',consumerIds:['prompts.registry']}],
  ['/watch',{labelKey:'prompts.watch',descriptionKey:'prompts.watch.description',scopes:['installation'],visibility:'developer',effect:'hot',consumerIds:['prompts.registry']}],
  ...Object.keys(promptsDefaults.limits).map(key=>[`/limits/${key}`,{labelKey:`prompts.limits.${key}`,descriptionKey:`prompts.limits.${key}.description`,scopes:['installation','subject'],visibility:'advanced',effect:'next_call',restriction:'upper_bound',consumerIds:['prompts.registry']}]),
] as [string,FieldMetadata][])};
