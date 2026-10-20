import { readFile,writeFile,unlink } from 'node:fs/promises';
import type { CallContext } from '@nous-wave/contracts/common';
import { ModelResourceSchema } from '@nous-wave/contracts/management';
import { authorize } from '../platform/security.js';
import { fail } from '../platform/errors.js';
import { json,now,type Storage } from '../platform/context.js';
import { secretFile } from '../platform/config.js';
import {createSettings} from './settings.js';
export function createManagement(store:Storage & {dataRoot:string}) {
  const settings=createSettings(store);
  const models={async register(ctx:CallContext,input:unknown){authorize(ctx,'models.manage');const body=ModelResourceSchema.parse(input);await store.transaction(async()=>{await store.db.insertInto('modelResources').values({id:body.id,body:json(body),recordedAt:now().at}).onConflict(oc=>oc.column('id').doUpdateSet({body:json(body),recordedAt:now().at})).execute();});return body;},async read(ctx:CallContext,id:string){authorize(ctx,'models.read');const row=await store.db.selectFrom('modelResources').select('body').where('id','=',id).executeTakeFirst();if(!row)fail('not_found','模型资源不存在');return ModelResourceSchema.parse(row.body);},async list(ctx:CallContext){authorize(ctx,'models.read');return (await store.db.selectFrom('modelResources').select('body').execute()).map(r=>ModelResourceSchema.parse(r.body));}};
  const secrets={async set(ctx:CallContext,name:string,value:string){authorize(ctx,'secrets.manage');await writeFile(await secretFile(store.dataRoot,name),value,{mode:0o600});return {configured:true};},async status(ctx:CallContext,name:string){authorize(ctx,'secrets.manage');try{await readFile(await secretFile(store.dataRoot,name));return {configured:true,source:'file'};}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;return {configured:false};}},async delete(ctx:CallContext,name:string){authorize(ctx,'secrets.manage');await unlink(await secretFile(store.dataRoot,name));return {configured:false};},async resolve(ref:string){if(ref.startsWith('env:')){const value=process.env[ref.slice(4)];if(!value)fail('dependency_unavailable','模型凭据未配置');return value;}if(!ref.startsWith('file:'))fail('invalid_input','不支持的秘密引用');return readFile(await secretFile(store.dataRoot,ref.slice(5)),'utf8');}};
  const costs={async read(ctx:CallContext){authorize(ctx,'costs.read');return store.db.selectFrom('modelInvocations').selectAll().where('subjectId','=',ctx.subjectId).orderBy('startedAt','desc').limit(500).execute();}};
  return {settings,models,secrets,costs};
}
