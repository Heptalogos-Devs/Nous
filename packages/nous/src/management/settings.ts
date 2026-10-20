import deepmerge from '@fastify/deepmerge';
import {sql} from 'kysely';
import {v7 as uuid} from 'uuid';
import {z} from 'zod';
import type {CallContext} from '@nous-wave/contracts/common';
import {BudgetSchema,SettingsSchema,SettingsPatchSchema,type Settings} from '@nous-wave/contracts/management';
import {authorize} from '../platform/security.js';
import {fail} from '../platform/errors.js';
import {json,now,type Storage} from '../platform/context.js';

type Tree=Record<string,unknown>;
type Scope='installation'|'subject'|'role';
type Selection={scope:Scope;role?:string};
type Update=Selection&{values:Tree;expectedRevision?:number;reason?:string;reset?:boolean};
export interface BudgetConstraint {scope:Scope;scopeId:string;limits:Partial<Settings['budget']>}
const merge=deepmerge({mergeArray:({clone})=>(_target,source)=>clone(source)});
function safePaths(value:unknown):void {
  if(!value||typeof value!=='object')return;
  for(const [key,item]of Object.entries(value)){
    if(['__proto__','constructor','prototype'].includes(key))fail('invalid_input','设置键无效');
    safePaths(item);
  }
}
export function createSettings(store:Storage) {
  function scopeId(ctx:CallContext,selection:Selection) {
    if(selection.scope==='installation')return store.installationId;
    if(!ctx.subjectId)fail('invalid_input','主体设置需要 subjectId');
    if(selection.scope==='role'&&!selection.role)fail('invalid_input','角色设置需要 role');
    return selection.scope==='role'?`${ctx.subjectId}:${selection.role}`:ctx.subjectId;
  }
  async function layers(ctx:CallContext,role?:string) {
    const scopes:Array<[Scope,string]>=[['installation',store.installationId],...(ctx.subjectId?[['subject',ctx.subjectId] as [Scope,string]]:[]),...(role&&ctx.subjectId?[['role',`${ctx.subjectId}:${role}`] as [Scope,string]]:[])];
    const result=[];for(const [scope,id]of scopes)result.push({scope,scopeId:id,row:await store.db.selectFrom('settingsOverrides').selectAll().where('scope','=',scope).where('scopeId','=',id).executeTakeFirst()});return result;
  }
  function combine(rows:Awaited<ReturnType<typeof layers>>) {
    let values:Tree={};const sources:Record<string,unknown>={},constraints:BudgetConstraint[]=[];
    const mark=(value:Tree,path:string[],source:unknown)=>{for(const [key,item]of Object.entries(value)){const next=[...path,key];if(item&&typeof item==='object'&&!Array.isArray(item))mark(item as Tree,next,source);else sources[next.join('.')]=source;}};
    for(const {scope,scopeId: id,row}of rows){if(!row)continue;const body=row.body as Tree;values=merge(values,body);mark(body,[],{scope,scopeId:id,revision:row.revision});if(body.budget)constraints.push({scope,scopeId:id,limits:body.budget as Partial<Settings['budget']>});}
    const effective=SettingsSchema.parse(values);
    for(const key of Object.keys(BudgetSchema.shape) as Array<keyof Settings['budget']>){
      const capped=constraints.flatMap(c=>typeof c.limits[key]==='number'?[{value:c.limits[key] as number,scope:c.scope,scopeId:c.scopeId}]:[]).sort((a,b)=>a.value-b.value)[0];
      if(capped){(effective.budget as Record<string,number|null>)[key]=capped.value;sources[`budget.${key}`]={scope:capped.scope,scopeId:capped.scopeId};}
    }
    if(!constraints.length)constraints.push({scope:ctxScope(rows),scopeId:rows.at(-1)!.scopeId,limits:effective.budget});
    return {effective,overrides:rows.flatMap(r=>r.row??[]),sources,constraints,effect:'next_call' as const};
  }
  function ctxScope(rows:Awaited<ReturnType<typeof layers>>):Scope{return rows.at(-1)!.scope;}
  async function read(ctx:CallContext,role?:string){authorize(ctx,'settings.read');return combine(await layers(ctx,role));}
  async function save(ctx:CallContext,input:Selection&{expectedRevision?:number;reason?:string},change:(old:Tree)=>Tree) {
    authorize(ctx,input.scope==='installation'?'installation.manage':'settings.update');const id=scopeId(ctx,input);
    return store.transaction(async()=>{
      await sql`select pg_advisory_xact_lock(hashtext(${`nous:settings:${input.scope}:${id}`}))`.execute(store.db);
      const old=await store.db.selectFrom('settingsOverrides').selectAll().where('scope','=',input.scope).where('scopeId','=',id).forUpdate().executeTakeFirst();
      if(input.expectedRevision!==undefined&&(old?.revision??0)!==input.expectedRevision)fail('conflict','设置已有新修订',{currentRevision:old?.revision??0});
      const before=(old?.body??{}) as Tree,proposed=change(structuredClone(before));safePaths(proposed);
      const body=SettingsPatchSchema.parse(proposed),revision=(old?.revision??0)+1;
      if(input.scope==='role'){
        if(body.growth||body.retrieval)fail('invalid_input','成长与检索设置仅在安装或主体范围配置');
        if(body.roles&&Object.keys(body.roles).some(role=>role!==input.role))fail('invalid_input','角色范围只能配置当前角色');
      }
      const proposedLayers=await layers(ctx,input.role);const target=proposedLayers.find(row=>row.scope===input.scope&&row.scopeId===id)!;
      target.row={scope:input.scope,scopeId:id,revision,body,recordedAt:now().at};combine(proposedLayers);
      const at=now().at;
      await store.db.insertInto('settingsOverrides').values({scope:input.scope,scopeId:id,revision,body:json(body),recordedAt:at}).onConflict(oc=>oc.columns(['scope','scopeId']).doUpdateSet({revision,body:json(body),recordedAt:at})).execute();
      await store.db.insertInto('settingsHistory').values({id:uuid(),scope:input.scope,scopeId:id,revision,body:json(body),previousBody:json(before),reason:input.reason??null,actor:json(ctx.actor),recordedAt:at}).execute();
      await sql`select pg_notify('nous_settings',${JSON.stringify({scope:input.scope,scopeId:id,revision})})`.execute(store.db);
      return {...await read(ctx,input.role),revision,runningRevision:revision};
    });
  }
  return {
    describe:()=>z.toJSONSchema(SettingsSchema),read,
    async update(ctx:CallContext,input:Update){safePaths(input.values);const values=SettingsPatchSchema.parse(input.values);return save(ctx,input,old=>input.reset?{}:merge(old,values));},
    async reset(ctx:CallContext,input:Selection&{paths?:string[][];expectedRevision?:number;reason?:string}){
      return save(ctx,input,old=>{
        if(!input.paths)return {};
        for(const path of input.paths){let parent:Tree|undefined=old;for(const part of path.slice(0,-1)){if(['__proto__','constructor','prototype'].includes(part))fail('invalid_input','设置路径无效');parent=parent?.[part] as Tree|undefined;}const key=path.at(-1);if(!key||['__proto__','constructor','prototype'].includes(key))fail('invalid_input','设置路径无效');if(parent)delete parent[key];}
        return old;
      });
    },
    async history(ctx:CallContext,input:Partial<Selection>={}){
      authorize(ctx,'settings.read');const visible=await layers(ctx,input.role);
      const rows=await store.db.selectFrom('settingsHistory').selectAll().where(eb=>eb.or(visible.filter(row=>!input.scope||row.scope===input.scope).map(row=>eb.and([eb('scope','=',row.scope),eb('scopeId','=',row.scopeId)])))).orderBy('recordedAt','desc').limit(200).execute();return rows;
    },
    async restore(ctx:CallContext,input:Selection&{revision:number;expectedRevision?:number;reason?:string}){
      authorize(ctx,input.scope==='installation'?'installation.manage':'settings.update');
      const prior=await store.db.selectFrom('settingsHistory').select('body').where('scope','=',input.scope).where('scopeId','=',scopeId(ctx,input)).where('revision','=',input.revision).executeTakeFirst();
      if(!prior)fail('not_found','设置历史修订不存在');return save(ctx,input,()=>prior.body as Tree);
    },
  };
}
