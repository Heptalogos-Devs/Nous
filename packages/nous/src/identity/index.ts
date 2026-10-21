import {requireSubject} from '../platform/security.js';
import {parseInstant} from '@nous-wave/platform/time';
import { z } from 'zod';
import { v7 as uuid } from 'uuid';
import type { CallContext } from '@nous-wave/contracts/common';
import { KnowledgeKindSchema } from '@nous-wave/contracts/knowledge';
import { SubjectCreateSchema } from '@nous-wave/contracts/identity';
import { fail } from '../platform/errors.js';
import { json, now, reference, type Storage } from '../platform/context.js';

export function createIdentity(store:Storage) {
  async function read(ctx:CallContext,db=store.db) {requireSubject(ctx);
    await store.authorize(ctx,'subjects.read',reference(store.installationId,ctx.subjectId,'Subject',ctx.subjectId));
    const row=await db.selectFrom('subjects').selectAll().where('id','=',ctx.subjectId).executeTakeFirst();
    if(!row) fail('not_found','主体不存在');
    return {...row,createdAt:parseInstant(row.createdAt),ref:reference(store.installationId,row.id,'Subject',row.id)};
  }
  return {
    async create(ctx:CallContext,input:z.input<typeof SubjectCreateSchema>) {
      const body=SubjectCreateSchema.parse(input),id=body.id??ctx.subjectId??uuid();
      if(ctx.subjectId&&ctx.subjectId!==id)fail('conflict','创建目标与当前主体范围不符');
      const ref=reference(store.installationId,id,'Subject',id);
      await store.authorize(ctx,'subjects.create',ref);
      return store.transaction(async db=>{
        const existing=await db.selectFrom('subjects').selectAll().where('id','=',id).executeTakeFirst();
        if(existing){if(existing.displayName!==body.displayName||JSON.stringify(existing.capabilities)!==JSON.stringify(body.capabilities))fail('conflict','同一主体身份已有不同内容');return {...existing,ref};}
        const row=await db.insertInto('subjects').values({id,displayName:body.displayName,capabilities:json(body.capabilities),status:'paused',createdAt:now().at,branchedFrom:null}).returningAll().executeTakeFirstOrThrow();
        return {...row,ref};
      });
    },read,
    async list(ctx:CallContext) {
      await store.authorize(ctx,'subjects.read');
      let query=store.db.selectFrom('subjects').selectAll();if(ctx.subjectId)query=query.where('id','=',ctx.subjectId);
      const rows=await query.execute(),items=[];
      for(const row of rows){
        const ref=reference(store.installationId,row.id,'Subject',row.id);
        try{await store.authorize(ctx,'subjects.read',ref);items.push({...row,ref});}
        catch(error){if(error&&typeof error==='object'&&'problem'in error&&(error.problem as {category?:string}).category==='forbidden')continue;throw error;}
      }
      return items;
    },
    async bind(ctx:CallContext,input:{installationId:string;subjectId:string;connectionRef:string}) {requireSubject(ctx);
      await store.authorize(ctx,'subjects.bind',reference(store.installationId,ctx.subjectId,'Subject',ctx.subjectId));if(input.subjectId!==ctx.subjectId)fail('conflict','绑定必须使用同一逻辑主体身份');
      await read(ctx);return store.transaction(async (db)=>{
        await db.insertInto('subjectBindings').values({id:uuid(),subjectId:ctx.subjectId,installationId:input.installationId,remoteSubjectId:input.subjectId,connectionRef:input.connectionRef,recordedAt:now().at}).onConflict(oc=>oc.columns(['subjectId','installationId']).doUpdateSet({connectionRef:input.connectionRef})).execute();
        return {subjectId:ctx.subjectId,installationId:input.installationId};
      });
    },
    async changes(ctx:CallContext,cursor='0') {requireSubject(ctx);await store.authorize(ctx,'subjects.read');return store.db.selectFrom('subjectChanges').selectAll().where('subjectId','=',ctx.subjectId).where('cursor','>',cursor).orderBy('cursor').limit(200).execute();},
    async provision(ctx:CallContext,capabilities:string[]) {requireSubject(ctx);await store.authorize(ctx,'subjects.provision',reference(store.installationId,ctx.subjectId,'Subject',ctx.subjectId));const parsed=z.array(KnowledgeKindSchema).parse(capabilities);await store.transaction(async (db)=>{await db.updateTable('subjects').set({capabilities:json(parsed)}).where('id','=',ctx.subjectId).execute();});return read(ctx);},
  };
}
