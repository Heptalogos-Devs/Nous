import { z } from 'zod';
import { v7 as uuid } from 'uuid';
import type { CallContext } from '@nous-wave/contracts/common';
import { KnowledgeKindSchema } from '@nous-wave/contracts/knowledge';
import { SubjectCreateSchema } from '@nous-wave/contracts/identity';
import { authorize } from '../platform/security.js';
import { fail } from '../platform/errors.js';
import { json, now, reference, type Storage } from '../platform/context.js';

export function createIdentity(store:Storage) {
  async function read(ctx:CallContext) {
    authorize(ctx,'subjects.read');
    const row=await store.db.selectFrom('subjects').selectAll().where('id','=',ctx.subjectId).executeTakeFirst();
    if(!row) fail('not_found','主体不存在');
    return {...row,createdAt:new Date(row.createdAt).toISOString(),ref:reference(store.installationId,row.id,'Subject',row.id)};
  }
  return {
    async create(ctx:CallContext,input:z.input<typeof SubjectCreateSchema>) {
      authorize(ctx,'subjects.create');const body=SubjectCreateSchema.parse(input);const id=body.id??uuid();
      if(ctx.actor.subjectIds&&!ctx.actor.subjectIds.includes('*')&&!ctx.actor.subjectIds.includes(id)) fail('forbidden','主体身份不在创建授权内');
      return store.transaction(async()=>{
        const existing=await store.db.selectFrom('subjects').selectAll().where('id','=',id).executeTakeFirst();
        if(existing) { if(existing.displayName!==body.displayName||JSON.stringify(existing.capabilities)!==JSON.stringify(body.capabilities)) fail('conflict','同一主体身份已有不同内容');return {...existing,ref:reference(store.installationId,id,'Subject',id)}; }
        await store.db.insertInto('subjects').values({id,displayName:body.displayName,capabilities:json(body.capabilities),status:'paused',createdAt:now().at,branchedFrom:null}).execute();
        return read({...ctx,subjectId:id});
      });
    },read,
    async list(ctx:CallContext) {authorize(ctx,'subjects.read');let q=store.db.selectFrom('subjects').selectAll();if(ctx.actor.subjectIds&&!ctx.actor.subjectIds.includes('*'))q=q.where('id','in',ctx.actor.subjectIds);return q.execute();},
    async bind(ctx:CallContext,input:{installationId:string;subjectId:string;connectionRef:string}) {
      authorize(ctx,'subjects.bind');if(input.subjectId!==ctx.subjectId)fail('conflict','绑定必须使用同一逻辑主体身份');
      await read(ctx);return store.transaction(async()=>{
        await store.db.insertInto('subjectBindings').values({id:uuid(),subjectId:ctx.subjectId,installationId:input.installationId,remoteSubjectId:input.subjectId,connectionRef:input.connectionRef,recordedAt:now().at}).onConflict(oc=>oc.columns(['subjectId','installationId']).doUpdateSet({connectionRef:input.connectionRef})).execute();
        return {subjectId:ctx.subjectId,installationId:input.installationId};
      });
    },
    async changes(ctx:CallContext,cursor='0') {authorize(ctx,'subjects.read');return store.db.selectFrom('subjectChanges').selectAll().where('subjectId','=',ctx.subjectId).where('cursor','>',cursor).orderBy('cursor').limit(200).execute();},
    async provision(ctx:CallContext,capabilities:string[]) {authorize(ctx,'subjects.provision');const parsed=z.array(KnowledgeKindSchema).parse(capabilities);await store.transaction(async()=>{await store.db.updateTable('subjects').set({capabilities:json(parsed)}).where('id','=',ctx.subjectId).execute();});return read(ctx);},
  };
}
