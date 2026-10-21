import type { z } from 'zod';
import type { CallContext } from '@nous-wave/contracts/common';
import type { PromptScope, PromptSnapshot } from '@nous-wave/contracts/prompts';
export interface PromptDescriptor<T=unknown> {
  id:string;owner:string;labelKey:string;descriptionKey:string;visibility:'standard'|'advanced'|'developer';
  locales:readonly string[];defaultLocale:string;programFiles:Readonly<Record<string,string>>;
  paramsSchema:z.ZodType<T>;paramsSchemaRevision:string;examples:Readonly<Record<string,T>>;allowedPartials:readonly string[];
}
export type PromptAction='describe'|'list'|'read'|'save'|'activate'|'reset'|'preview'|'snapshot'|'render'|'history'|'export'|'watch';
export type PromptAuthorize=(ctx:CallContext,input:{action:PromptAction;promptId?:string;scope:PromptScope})=>Promise<void>|void;
export interface PromptSource {promptId:string;locale:string;scope:PromptScope;source:'program'|'override';revision?:number;digest:string;text:string}
export interface PromptManifest {
  snapshot:PromptSnapshot; sources:PromptSource[];limits:PromptLimits;
  paramsSchemaDigest:string;
  settings?:{definitionRevision:string;revisions:unknown[]};
}
export interface PromptLimits {sourceBytes:number;closureBytes:number;outputBytes:number;closureCount:number;renderMs:number;memoryBytes:number}
export type PromptDescription=z.infer<typeof import('@nous-wave/contracts/prompts').PromptDescriptionSchema>;
