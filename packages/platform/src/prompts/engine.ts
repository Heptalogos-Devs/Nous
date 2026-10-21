// Prompt cloning, nested-block validation and cycle rejection ported from
// MaiBot ff0fe425 src/prompt/prompt_manager.py PromptManager._render.
// Copyright MaiBot contributors; SPDX-License-Identifier: GPL-3.0-only
// LiquidJS replaces Formatter and owns parsing/rendering; params are prepared by the domain.
import {Liquid,toPromise} from 'liquidjs';
import type {Template} from 'liquidjs';
import {z} from 'zod';
import {canonicalize} from 'json-canonicalize';
import {problem} from '../errors.js';
import {assertJson,immutable,isObject} from '../settings/catalog.js';
import {digest,checkText} from './registry.js';
import type {PromptDescriptor,PromptLimits,PromptSource} from './types.js';
export const ENGINE_VERSION='10.30.0';
export function engineOptions(limits:PromptLimits){return {dynamicPartials:false,relativeReference:false,strictVariables:true,strictFilters:true,ownPropertyOnly:true,parseLimit:limits.sourceBytes,renderLimit:limits.renderMs,memoryLimit:limits.memoryBytes};}
export function optionsDigest(limits:PromptLimits){return digest(canonicalize(engineOptions(limits)));}
function renderer(limits:PromptLimits,sources:PromptSource[]){return new Liquid({...engineOptions(limits),templates:Object.fromEntries(sources.map(source=>[source.promptId,source.text]))});}
export async function partials(text:string,limits:PromptLimits):Promise<string[]>{
  try{
    const engine=renderer(limits,[]);const pending:Template[]=engine.parse(text);const found=new Set<string>();
    while(pending.length){const node=pending.pop()!;const partial=node.partialScope?.();if(partial)found.add(partial.name);if(node.children)pending.push(...await toPromise(node.children(false,false)));}
    return [...found];
  }catch{throw problem('prompts.invalid_template','validation','Prompt模板语法无效');}
}
export async function closure(root:PromptSource,catalog:Map<string,PromptDescriptor>,load:(id:string)=>Promise<PromptSource>,limits:PromptLimits):Promise<PromptSource[]>{
  const sources=new Map<string,PromptSource>();const visiting=new Set<string>();let bytes=0;
  async function visit(source:PromptSource){
    if(visiting.has(source.promptId))throw problem('prompts.dependency_cycle','validation','Prompt依赖存在循环');
    if(sources.has(source.promptId))return;
    if(sources.size+visiting.size>=limits.closureCount)throw problem('prompts.closure_limit','validation','Prompt依赖数量超限');
    const descriptor=catalog.get(source.promptId);if(!descriptor)throw problem('prompts.partial_not_found','not_found','Prompt依赖未注册');
    checkText(source.text,limits.sourceBytes);bytes+=Buffer.byteLength(source.text);if(bytes>limits.closureBytes)throw problem('prompts.closure_limit','validation','Prompt依赖字节超限');
    visiting.add(source.promptId);
    for(const id of await partials(source.text,limits)){
      if(!descriptor.allowedPartials.includes(id))throw problem('prompts.partial_forbidden','forbidden','Prompt使用了未声明的依赖');
      await visit(await load(id));
    }
    visiting.delete(source.promptId);sources.set(source.promptId,immutable(structuredClone(source)));
  }
  await visit(root);return [...sources.values()];
}
export async function renderSources(descriptor:PromptDescriptor,sources:PromptSource[],params:unknown,limits:PromptLimits):Promise<{text:string;bytes:number}>{
  const parsed=descriptor.paramsSchema.safeParse(params);
  if(!parsed.success)throw problem('prompts.invalid_params','validation','Prompt参数不符合领域合同',{details:{issues:parsed.error.issues.map(issue=>({path:issue.path.map(String),code:issue.code}))}});
  assertJson(parsed.data);
  if(!isObject(parsed.data))throw problem('prompts.invalid_params','validation','Prompt参数必须是普通JSON对象');
  try{
    const source=sources.find(source=>source.promptId===descriptor.id)!;
    const engine=renderer(limits,sources);const templates=engine.parse(source.text);
    const text=await engine.render(templates,structuredClone(parsed.data));const bytes=Buffer.byteLength(text,'utf8');
    if(bytes>limits.outputBytes)throw problem('prompts.output_limit','validation','Prompt渲染结果字节超限');
    return {text,bytes};
  }catch(error){if(error&&typeof error==='object'&&'problem'in error)throw error;throw problem('prompts.render_failed','validation','Prompt渲染失败，请检查变量、过滤器与资源限制');}
}
export async function validateExamples(descriptor:PromptDescriptor,sources:PromptSource[],locale:string,limits:PromptLimits){
  const example=descriptor.examples[locale]??descriptor.examples[descriptor.defaultLocale];
  if(example===undefined)throw problem('prompts.example_required','validation','Prompt定义缺少参数样例');
  await renderSources(descriptor,sources,example,limits);
}
