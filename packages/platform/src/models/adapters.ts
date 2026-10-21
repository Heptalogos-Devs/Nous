// Thin composition of the official AI SDK 7 Provider implementations.
import {z} from 'zod';
import {createOpenAI} from '@ai-sdk/openai';
import {createCohere} from '@ai-sdk/cohere';
import {createOpenAICompatible} from '@ai-sdk/openai-compatible';
import type {ProviderAdapter} from './types.js';
import {problem} from '../errors.js';
const routing={addressScope:z.enum(['public','configured_private']).optional(),allowedPrivateRanges:z.array(z.string()).optional()};
const openAIOptions=z.strictObject({...routing,mode:z.enum(['responses','chat','completion']).optional(),organization:z.string().optional(),project:z.string().optional()});
const compatibleOptions=z.strictObject({...routing,supportsStructuredOutputs:z.boolean().optional()});
const cohereOptions=z.strictObject({...routing});
function networkScope(defaultBaseURL:string):ProviderAdapter['networkScope']{
 return ({provider,resourceRef})=>{
  const base=new URL(provider.baseURL??defaultBaseURL);
  if(!['http:','https:'].includes(base.protocol)||base.username||base.password||base.search||base.hash)throw problem('models.invalid_base_url','validation','模型服务地址必须是无秘密的HTTP端点');
  return {resourceRef,operation:'models.invoke',profileId:provider.networkProfileId,allowedOrigins:[base.origin],allowedPathPrefixes:[base.pathname.endsWith('/')?base.pathname:base.pathname+'/'],methods:['POST','GET'],addressScope:provider.options.addressScope==='configured_private'?'configured_private':'public',...(Array.isArray(provider.options.allowedPrivateRanges)?{allowedPrivateRanges:provider.options.allowedPrivateRanges as string[]}:{}),credentialOrigins:[base.origin]};
 };
}
export const officialProviderAdapters:readonly ProviderAdapter[]=[
 {id:'openai',optionsSchema:openAIOptions,defaultBaseURL:'https://api.openai.com/v1',networkScope:networkScope('https://api.openai.com/v1'),reasoning:(_provider,reasoningEffort)=>({openai:{reasoningEffort}}),create({provider,apiKey,fetch}){
  const options=openAIOptions.parse(provider.options);const sdk=createOpenAI({baseURL:provider.baseURL,apiKey:apiKey??'',fetch,organization:options.organization,project:options.project});
  return {...sdk,languageModel:(id:string)=>options.mode==='chat'?sdk.chat(id):options.mode==='completion'?sdk.completion(id):sdk.responses(id)};
 }},
 {id:'openai-compatible',optionsSchema:compatibleOptions,networkScope:networkScope('https://invalid.invalid/'),reasoning:(_provider,reasoningEffort)=>({openaiCompatible:{reasoningEffort}}),create({provider,apiKey,fetch}){
  if(!provider.baseURL)throw problem('models.base_url_required','validation','兼容Provider需要明确baseURL');
  const options=compatibleOptions.parse(provider.options);return createOpenAICompatible({name:provider.id,baseURL:provider.baseURL,apiKey:apiKey??'',fetch,supportsStructuredOutputs:options.supportsStructuredOutputs});
 }},
 {id:'cohere',optionsSchema:cohereOptions,defaultBaseURL:'https://api.cohere.com/v2',networkScope:networkScope('https://api.cohere.com/v2'),create({provider,apiKey,fetch}){cohereOptions.parse(provider.options);return createCohere({baseURL:provider.baseURL,apiKey:apiKey??'',fetch});},usage(kind,value){
  if(kind!=='reranking')return {};
  // Cohere v2 returns actual billed_units in response.body.meta, not V4 usage.
  // https://docs.cohere.com/v2/reference/rerank
  const raw=(value as {response?:{body?:{meta?:{billed_units?:unknown}}}})?.response?.body?.meta?.billed_units;
  if(!raw||typeof raw!=='object'||Array.isArray(raw))return {};
  const units=Object.entries(raw).flatMap(([unit,quantity])=>typeof quantity==='number'&&Number.isFinite(quantity)&&quantity>=0?[{unit:`cohere.${unit}`,quantity:String(quantity),source:'provider' as const}]:[]);
  return {rawUsage:raw,usage:{units}};
 }},
];
