// Managed editing/preview workflow ported from MaiBot PromptManager and WebUI prompt routes.
// Copyright MaiBot contributors; SPDX-License-Identifier: GPL-3.0-only
import {mkdir} from 'node:fs/promises';
import {dirname,join,relative,resolve,sep} from 'node:path';
import {watch as chokidarWatch} from 'chokidar';
import type {FSWatcher} from 'chokidar';
import writeFileAtomic from 'write-file-atomic';
import {z} from 'zod';
import type {CallContext,Problem} from '@nous-wave/contracts/common';
import {PromptIdSchema,PromptLocaleSchema,PromptScopeSchema} from '@nous-wave/contracts/prompts';
import type {PromptScope,PromptSave,PromptActivate,PromptRead,PromptReadResult,PromptChangeResult} from '@nous-wave/contracts/prompts';
import {problem,safeProblem} from '../errors.js';
import {digest as jsonDigest} from '../settings/catalog.js';
import type {Settings,ConfigSnapshot} from '../settings/index.js';
import {checkText,type PromptRegistry} from './registry.js';
import type {PromptsSettingsSchema} from './config.js';
const metadataSchema=z.strictObject({promptId:PromptIdSchema,locale:PromptLocaleSchema,scope:PromptScopeSchema,baseRevision:z.number().int().nonnegative().safe(),digest:z.string().regex(/^[a-f0-9]{64}$/)});
interface Options {
  installationId:string;overrideRoot:string;registry:PromptRegistry;settings:Settings;
  api:{read(ctx:CallContext,input:PromptRead):Promise<PromptReadResult>;save(ctx:CallContext,input:PromptSave):Promise<PromptChangeResult>;activate(ctx:CallContext,input:PromptActivate):Promise<PromptChangeResult>};
  config(scope:PromptScope):Promise<{snapshot:ConfigSnapshot;value:z.infer<typeof PromptsSettingsSchema>;consumer:ReturnType<Settings['registerConsumer']>}>;
  watchContext?:()=>Promise<CallContext>;onError?:(problem:Problem)=>void;
}
export function createEditor(options:Options){
  let watcher:FSWatcher|undefined,unsubscribe:(()=>void)|undefined,watchCtx:CallContext|undefined;
  let stopped=false;const jobs=new Map<string,Promise<void>>();const blocked=new Map<string,number>();
  let updates=Promise.resolve();
  const target:PromptScope={kind:'installation',installationId:options.installationId};
  function path(input:{promptId:string;locale:string;scope:PromptScope}){
    PromptIdSchema.parse(input.promptId);PromptLocaleSchema.parse(input.locale);PromptScopeSchema.parse(input.scope);
    const identity=input.scope.kind==='installation'?input.scope.installationId:input.scope.subjectId;
    return join(input.scope.kind,encodeURIComponent(identity),input.locale,`${input.promptId}.liquid`);
  }
  async function exportFile(ctx:CallContext,input:PromptRead){
    const selected=await options.api.read(ctx,input);const local=path(input);const file=join(options.overrideRoot,local);
    const metadata={promptId:input.promptId,locale:input.locale,scope:input.scope,baseRevision:selected.revision,digest:selected.digest};
    try{await mkdir(dirname(file),{recursive:true,mode:0o700});await writeFileAtomic(file,selected.text,{mode:0o600,fsync:true});await writeFileAtomic(file.replace(/\.liquid$/,'.meta.json'),JSON.stringify(metadata,null,2)+'\n',{mode:0o600,fsync:true});}
    catch{throw problem('prompts.export_failed','unavailable','Prompt编辑文件未同步，数据库版本仍可用');}
    return {path:local,metadata};
  }
  async function importFile(local:string){
    const bodyPath=local.endsWith('.meta.json')?local.replace(/\.meta\.json$/,'.liquid'):local;
    const metadataPath=bodyPath.replace(/\.liquid$/,'.meta.json');
    const metaAsset=await options.registry.load(metadataPath,'config');
    let meta:z.infer<typeof metadataSchema>;
    try{meta=metadataSchema.parse(JSON.parse(metaAsset.text));}catch{throw problem('prompts.invalid_edit_manifest','validation','Prompt编辑manifest无效');}
    if(path(meta).split(sep).join('/')!==bodyPath.split(sep).join('/'))throw problem('prompts.edit_identity_mismatch','validation','Prompt编辑路径与manifest身份不同');
    if(blocked.get(bodyPath)===meta.baseRevision)return;
    const cfg=await options.config(meta.scope);const asset=await options.registry.load(bodyPath,'config',cfg.value.limits.sourceBytes);
    if(checkText(asset.text,cfg.value.limits.sourceBytes)===meta.digest)return;
    const ctx=options.watchContext?await options.watchContext():watchCtx;
    if(!ctx)throw problem('prompts.watch_context_required','unavailable','Prompt自动编辑需要可信宿主身份');
    const operationId=`prompts.file:${jsonDigest({path:bodyPath,digest:asset.digest,baseRevision:meta.baseRevision})}`;
    const call=(part:string):CallContext=>({...ctx,trace:{...ctx.trace,operationId:`${operationId}:${part}`}});
    try{
      const saved=await options.api.save(call('save'),{promptId:meta.promptId,locale:meta.locale,scope:meta.scope,expectedRevision:meta.baseRevision,text:asset.text,reason:'local_edit'});
      await options.api.activate(call('activate'),{promptId:meta.promptId,locale:meta.locale,scope:meta.scope,expectedRevision:saved.revision,revision:saved.latestRevision});
      await exportFile(ctx,{promptId:meta.promptId,locale:meta.locale,scope:meta.scope});blocked.delete(bodyPath);
    }catch(error){if(error&&typeof error==='object'&&'code'in error&&error.code==='prompts.revision_conflict')blocked.set(bodyPath,meta.baseRevision);throw error;}
  }
  function changed(file:string){
    const local=relative(resolve(options.overrideRoot),resolve(file));
    if(local==='..'||local.startsWith(`..${sep}`)||!(/\.(liquid|meta\.json)$/.test(local))||stopped)return;
    const key=local.replace(/\.meta\.json$/,'.liquid');
    const job=(jobs.get(key)??Promise.resolve()).then(()=>importFile(local)).catch(error=>options.onError?.(safeProblem(error,'prompts.edit_failed'))).finally(()=>{if(jobs.get(key)===job)jobs.delete(key);});jobs.set(key,job);
  }
  async function apply(){
    if(stopped)return;
    const cfg=await options.config(target);
    try{
      if(cfg.value.watch.enabled&&!watcher){
        if(!options.watchContext&&!watchCtx)throw problem('prompts.watch_context_required','unavailable','Prompt自动编辑需要可信宿主身份');
        await mkdir(options.overrideRoot,{recursive:true,mode:0o700});
        watcher=chokidarWatch(options.overrideRoot,{ignoreInitial:true,awaitWriteFinish:{stabilityThreshold:200,pollInterval:50}});
        watcher.on('add',changed).on('change',changed).on('error',()=>{const error=problem('prompts.watch_failed','unavailable','Prompt编辑目录监视失败').problem;if(!stopped)cfg.consumer.reportFailure(error,['/watch']);options.onError?.(error);});
        await new Promise<void>((resolve,reject)=>{watcher!.once('ready',resolve);watcher!.once('error',()=>reject(problem('prompts.watch_failed','unavailable','Prompt编辑目录监视失败')));});
        // Chokidar's initial inventory includes edits made while this host was stopped.
        for(const [directory,files] of Object.entries(watcher.getWatched())){
          for(const file of files.filter(name=>name.endsWith('.meta.json')).sort())changed(join(directory,file));
        }
        await Promise.allSettled(jobs.values());
      }else if(!cfg.value.watch.enabled&&watcher){await watcher.close();watcher=undefined;}
      cfg.consumer.reportApplied(cfg.snapshot,['/watch']);
    }catch(error){cfg.consumer.reportFailure(safeProblem(error,'prompts.watch_failed'),['/watch']);throw error;}
  }
  function refresh(){return updates=updates.catch(()=>{}).then(apply);}
  async function watch(ctx?:CallContext){
    stopped=false;if(ctx)watchCtx=ctx;await refresh();
    if(!unsubscribe)unsubscribe=options.settings.subscribe(event=>{if(event.kind==='resync'||(event.change.namespace==='platform.prompts'&&event.change.scope.kind==='installation'))void refresh().catch(error=>options.onError?.(safeProblem(error,'prompts.watch_failed')));});
    return {enabled:Boolean(watcher),close};
  }
  async function close(){stopped=true;unsubscribe?.();unsubscribe=undefined;await updates.catch(()=>{});await watcher?.close();watcher=undefined;await Promise.allSettled(jobs.values());blocked.clear();}
  return {exportFile,watch,close};
}
