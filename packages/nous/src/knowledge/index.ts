import { createHash, randomBytes } from 'node:crypto';
import proquint from 'proquint';
import { v7 as uuid } from 'uuid';
import { sql } from 'kysely';
import { executeWithCursorPagination } from 'kysely-paginate';
import { z } from 'zod';
import { canonicalRefKey, type CallContext, type Ref, type TimePoint, type TimeRange,type Policy } from '@nous-wave/contracts/common';
import { KnowledgeCreateSchema, KnowledgeReviseSchema, KnowledgeReadSchema, KnowledgeListSchema, KnowledgeHistorySchema, RelationReadSchema, KnowledgeBodySchema, type KnowledgeView, type KnowledgePage } from '@nous-wave/contracts/knowledge';
import { authorize, intersectPolicy, privatePolicy } from '../platform/security.js';
import { fail, DomainError } from '../platform/errors.js';
import { changed, json, now, reference, type Storage } from '../platform/context.js';
import type { createSourceRepository } from '../materials/source-repository.js';
import { createKnowledgeRepository, type KnowledgeRecord, type ReadRequest } from './repository.js';
import { createSourceResolver, type SourceCutoff } from './sources.js';

export function createKnowledge(store: Storage & { sourceRepository: ReturnType<typeof createSourceRepository>;sourcePolicies?:(ctx:CallContext,roots:Ref[])=>Promise<Policy[]> }) {
  const repository = createKnowledgeRepository(store);
  const sources = createSourceResolver(store, repository, store.sourceRepository);
  const scopeFor = (ctx: CallContext, operation: string, input: unknown) => createHash('sha256').update(JSON.stringify({operation,subjectId:ctx.subjectId,actor:ctx.actor,purpose:ctx.purpose,audience:[...ctx.audience].sort(),input})).digest('hex');
  function view(record: KnowledgeRecord, policies: Parameters<typeof intersectPolicy>[0], roots: Ref[]): KnowledgeView {
    const {object,revision:r}=record;
    return {kind:KnowledgeBodySchema.parse(r.body).kind,body:r.body,sourceRefs:r.sourceRefs,formed:r.formed,valid:r.valid??undefined,formation:KnowledgeCreateSchema.shape.formation.parse(r.formation),stance:KnowledgeCreateSchema.shape.stance.parse(r.stance),confidence:r.confidence,policy:intersectPolicy(policies),ref:reference(store.installationId,object.subjectId,object.kind,object.id,r.revision),alias:object.alias,revision:r.revision,sourceRoots:roots,recorded:{clockId:'real',at:new Date(r.recordedAt).toISOString()},status:object.status as KnowledgeView['status']};
  }
  async function selected(ctx:CallContext, records:KnowledgeRecord[],cutoff:SourceCutoff={},strict=false):Promise<KnowledgeView[]> {
    const refs=records.map(record=>reference(store.installationId,ctx.subjectId,record.object.kind,record.object.id,record.revision.revision));
    const resolutions=await sources.resolveMany(ctx,refs,cutoff,records),items:KnowledgeView[]=[];
    for(let index=0;index<records.length;index++) {
      const result=resolutions.get(canonicalRefKey(refs[index]!))!;
      if(result instanceof DomainError) {if(!strict&&['not_found','forbidden'].includes(result.code))continue;throw result;}
      if(!result.known){if(strict)fail('not_found','该截点尚未实际取得形成所用的来源');continue;}
      items.push(view(records[index]!,result.policies,result.roots));
    }
    return items;
  }
  async function read(ctx:CallContext,input:ReadRequest):Promise<KnowledgeView> {
    authorize(ctx,'knowledge.read');const request=KnowledgeReadSchema.parse(input),records=await repository.records(ctx,[request]);
    if(!records.length)fail('not_found','认识或所选截点不存在');
    return (await selected(ctx,records,request,true))[0]!;
  }
  async function readMany(ctx:CallContext,requests:ReadRequest[]):Promise<KnowledgeView[]> {
    authorize(ctx,'knowledge.read');if(requests.length>200)fail('invalid_input','批量读取最多200项');
    const parsed=requests.map(request=>KnowledgeReadSchema.parse(request));
    const groups=new Map<string,ReadRequest[]>();
    for(const request of parsed){const key=JSON.stringify({knowledgeAt:request.knowledgeAt,recordedAsOf:request.recordedAsOf});const group=groups.get(key)??[];group.push(request);groups.set(key,group);}
    const result:KnowledgeView[]=[];
    for(const group of groups.values())result.push(...await selected(ctx,await repository.records(ctx,group),group[0]));
    return result;
  }
  async function validate(ctx:CallContext,input:z.input<typeof KnowledgeCreateSchema>) {
    const body=KnowledgeCreateSchema.parse(input);if(body.body.kind!==body.kind)fail('invalid_input','认识 kind 与正文不一致');
    const subject=await store.db.selectFrom('subjects').select('capabilities').where('id','=',ctx.subjectId).executeTakeFirst();
    if(!subject)fail('not_found','主体不存在');if(!subject.capabilities.includes(body.kind))fail('capability_unavailable',`主体未启用 ${body.kind}`);
    if(!body.sourceRefs.length&&!['operator_edit','initial_setting'].includes(body.formation))fail('invalid_input','自然形成的认识需要实际来源');
    if(body.body.kind==='Motivation'&&body.body.aspect==='evaluable_expectation'&&!body.body.criterion)fail('invalid_input','可评价期望需要条件');
    if(body.body.kind==='Social'&&body.body.aspect==='relation'&&!body.body.toRef)fail('invalid_input','关系认识需要有方向的双方');
    const administrative=['operator_edit','initial_setting'].includes(body.formation);
    const resolved=await sources.resolve(ctx,body.sourceRefs,administrative?{}:{knowledgeAt:body.formed});
    if(!resolved.known)fail('invalid_input','自然形成时间早于实际取得的来源');
    body.sourceRefs=resolved.resolvedRefs;
    if(body.body.conceptRefs.length){const concepts=await readMany(ctx,body.body.conceptRefs.map(ref=>({id:ref.id,revision:ref.revision})));if(concepts.length!==body.body.conceptRefs.length||concepts.some(concept=>concept.kind!=='Concept'))fail('invalid_input','概念关联必须引用本主体可读的正式概念');body.body.conceptRefs=concepts.map(concept=>concept.ref);}
    return {body,sources:resolved,policy:intersectPolicy([...(body.policy?[body.policy]:resolved.policies.length?[]:[privatePolicy(ctx.subjectId)]),...resolved.policies])};
  }
  async function writeRevision(ctx:CallContext,id:string,revision:number,body:z.infer<typeof KnowledgeCreateSchema>,sourceRoots:Ref[],reason:string|null) {
    const revisionId=uuid();if(!sourceRoots.length&&['operator_edit','initial_setting'].includes(body.formation))sourceRoots=[reference(store.installationId,ctx.subjectId,body.formation==='operator_edit'?'OperatorEdit':'InitialSetting',revisionId)];
    await store.db.insertInto('knowledgeRevisions').values({id:revisionId,objectId:id,revision,body:json(body.body),sourceRefs:json(body.sourceRefs),sourceRoots:json(sourceRoots),formed:json(body.formed),valid:body.valid?json(body.valid):null,recordedAt:now().at,formation:body.formation,stance:body.stance,confidence:body.confidence,actor:json(ctx.actor),reason}).execute();
    const fromRef=reference(store.installationId,ctx.subjectId,body.kind,id,revision);
    if(body.sourceRefs.length)await store.db.insertInto('sourceLinks').values(body.sourceRefs.map(toRef=>({id:uuid(),subjectId:ctx.subjectId,fromRef:json(fromRef),toRef:json(toRef),relation:'supports',roots:json(sourceRoots)}))).onConflict(oc=>oc.columns(['subjectId','fromRef','toRef','relation']).doNothing()).execute();
    if(body.body.conceptRefs.length)await store.db.insertInto('knowledgeConcepts').values([...new Set(body.body.conceptRefs.map(ref=>ref.id))].map(conceptId=>({subjectId:ctx.subjectId,objectId:id,revision,conceptId}))).execute();
    await changed(store.db,ctx,[fromRef]);return read(ctx,{id,revision});
  }
  const operations={
    read,readMany,sources:{resolve:sources.resolve,inherit:sources.inherit,readable:sources.readable,externalPolicies:sources.externalPolicies},
    async list(ctx:CallContext,input:z.input<typeof KnowledgeListSchema>={}):Promise<KnowledgePage> {
      authorize(ctx,'knowledge.read');const request=KnowledgeListSchema.parse(input),{cursor,...filter}=request,scope=scopeFor(ctx,'list',filter);
      const page=await executeWithCursorPagination(repository.candidates(ctx,request,scope),{perPage:request.limit,after:cursor,cursorPerRow:true,fields:[{expression:sql<string>`${scope}`,key:'scope',direction:'asc'},{expression:'o.id',key:'id',direction:'asc'},{expression:'v.revision',key:'revision',direction:'asc'}],parseCursor:value=>{const revision=Number(value.revision);if(value.scope!==scope||!z.uuid().safeParse(value.id).success||!Number.isSafeInteger(revision)||revision<1)fail('invalid_input','游标不属于本次查询范围');return {...value,revision};}}).catch(error=>{if(error instanceof DomainError)throw error;fail('invalid_input','知识游标无效');});
      const records=await repository.records(ctx,page.rows.map(row=>({id:row.id,revision:row.revision}))),items=await selected(ctx,records,request);
      return {items,nextCursor:page.hasNextPage?page.endCursor:undefined};
    },
    async create(ctx:CallContext,input:z.input<typeof KnowledgeCreateSchema>):Promise<KnowledgeView> {
      authorize(ctx,'knowledge.create');return store.transaction(async()=>{const {body,sources:resolved,policy}=await validate(ctx,input);const id=uuid();let alias='';
        for(let attempt=0;attempt<8;attempt++){alias=proquint.encode(randomBytes(6));const inserted=await store.db.insertInto('readableRefs').values({subjectId:ctx.subjectId,alias,objectId:id,kind:body.kind}).onConflict(oc=>oc.columns(['subjectId','alias']).doNothing()).returning('alias').executeTakeFirst();if(inserted)break;alias='';}
        if(!alias)fail('dependency_unavailable','无法分配可读引用');
        await store.db.insertInto('knowledgeObjects').values({id,subjectId:ctx.subjectId,kind:body.kind,currentRevision:1,status:'active',policy:json(policy),alias,createdAt:now().at}).execute();return writeRevision(ctx,id,1,body,resolved.roots,null);
      });
    },
    async revise(ctx:CallContext,input:z.input<typeof KnowledgeReviseSchema>):Promise<KnowledgeView> {
      authorize(ctx,'knowledge.revise');const request=KnowledgeReviseSchema.parse(input);return store.transaction(async()=>{const selected=await read(ctx,{id:request.id});const old=await store.db.selectFrom('knowledgeObjects').selectAll().where('subjectId','=',ctx.subjectId).where('id','=',selected.ref.id).forUpdate().executeTakeFirstOrThrow();if(old.currentRevision!==request.expectedRevision)fail('conflict','认识已有新修订',{currentRevision:old.currentRevision});if(old.kind!==request.kind)fail('invalid_input','认识修订不能改变领域类型');
        const {body,sources:resolved,policy}=await validate(ctx,request);await store.db.updateTable('knowledgeObjects').set({currentRevision:old.currentRevision+1,policy:json(policy)}).where('id','=',old.id).execute();return writeRevision(ctx,old.id,old.currentRevision+1,body,resolved.roots,request.reason);
      });
    },
    async setStatus(ctx:CallContext,input:{id:string;status:'active'|'suppressed'|'retired';expectedRevision:number}) {authorize(ctx,'knowledge.revise');return store.transaction(async()=>{const old=await read(ctx,{id:input.id});const updated=await store.db.updateTable('knowledgeObjects').set({status:input.status}).where('id','=',old.ref.id).where('subjectId','=',ctx.subjectId).where('currentRevision','=',input.expectedRevision).returning('id').executeTakeFirst();if(!updated)fail('conflict','认识已有新修订');await changed(store.db,ctx,[old.ref]);return {...old,status:input.status};});},
    async history(ctx:CallContext,id:string,input:z.input<typeof KnowledgeHistorySchema>={}):Promise<KnowledgePage> {
      authorize(ctx,'knowledge.read');const chosen=await read(ctx,{id}),request=KnowledgeHistorySchema.parse(input),{cursor,...filter}=request,scope=scopeFor(ctx,'history',{id:chosen.ref.id,...filter});
      let query=store.db.selectFrom('knowledgeRevisions').select(['objectId','revision']).select(sql<string>`${scope}`.as('scope')).where('objectId','=',chosen.ref.id);
      if(request.knowledgeAt)query=query.where(sql<boolean>`formed->>'clockId'=${request.knowledgeAt.clockId} and formed->>'at'<=${request.knowledgeAt.at}`);
      if(request.recordedAsOf)query=query.where('recordedAt','<=',request.recordedAsOf);
      const page=await executeWithCursorPagination(query,{perPage:request.limit,after:cursor,fields:[{expression:sql<string>`${scope}`,key:'scope',direction:'asc'},{expression:'revision',key:'revision',direction:'asc'}],parseCursor:value=>{const revision=Number(value.revision);if(value.scope!==scope||!Number.isSafeInteger(revision)||revision<1)fail('invalid_input','历史游标不属于本次查询');return {...value,revision};}});
      return {items:await selected(ctx,await repository.records(ctx,page.rows.map(row=>({id:chosen.ref.id,revision:row.revision}))),request),nextCursor:page.hasNextPage?page.endCursor:undefined};
    },
    async relate(ctx:CallContext,input:{fromId:string;toId:string;relation:string;weight?:number;evidenceRefs:Ref[];formed:TimePoint;valid?:TimeRange}) {authorize(ctx,'knowledge.revise');return store.transaction(async()=>{const from=await read(ctx,{id:input.fromId}),to=await read(ctx,{id:input.toId}),evidence=await sources.resolve(ctx,input.evidenceRefs);await sql`select pg_advisory_xact_lock(hashtextextended(${`${ctx.subjectId}:${from.ref.id}:${to.ref.id}:${input.relation}`},0))`.execute(store.db);const previous=await store.db.selectFrom('objectRelations').selectAll().where('subjectId','=',ctx.subjectId).where('fromId','=',from.ref.id).where('toId','=',to.ref.id).where('relation','=',input.relation).orderBy('revision','desc').limit(1).executeTakeFirst();const row={id:uuid(),relationId:previous?.relationId??uuid(),revision:(previous?.revision??0)+1,subjectId:ctx.subjectId,fromId:from.ref.id,toId:to.ref.id,relation:input.relation,weight:input.weight??1,evidenceRefs:json(evidence.resolvedRefs),roots:json(evidence.roots),formed:json(input.formed),valid:input.valid?json(input.valid):null,recordedAt:now().at};await store.db.insertInto('objectRelations').values(row).execute();await changed(store.db,ctx,[from.ref,to.ref]);return row;});},
    async relations(ctx:CallContext,ids:string[],input:z.input<typeof RelationReadSchema>={}) {
      authorize(ctx,'knowledge.read');const request=RelationReadSchema.parse(input);if(!ids.length)return {items:[],nextCursor:undefined};
      const endpoints=await readMany(ctx,ids.map(id=>({id,...request}))),resolvedIds=endpoints.map(item=>item.ref.id);if(!resolvedIds.length)return {items:[],nextCursor:undefined};
      const {cursor,...filter}=request,scope=scopeFor(ctx,'relations',{ids:[...resolvedIds].sort(),...filter});
      let revisions=store.db.selectFrom('objectRelations').selectAll().distinctOn('relationId').where('subjectId','=',ctx.subjectId).where(eb=>eb.or([eb('fromId','in',resolvedIds),eb('toId','in',resolvedIds)]));
      if(request.knowledgeAt)revisions=revisions.where(sql<boolean>`formed->>'clockId'=${request.knowledgeAt.clockId} and formed->>'at'<=${request.knowledgeAt.at}`);
      if(request.recordedAsOf)revisions=revisions.where('recordedAt','<=',request.recordedAsOf);
      const query=store.db.selectFrom(revisions.orderBy('relationId').orderBy('revision','desc').as('r')).selectAll().select(sql<string>`${scope}`.as('scope'));
      const page=await executeWithCursorPagination(query,{perPage:request.limit,after:cursor,fields:[{expression:sql<string>`${scope}`,key:'scope',direction:'asc'},{expression:'r.relationId',key:'relationId',direction:'asc'}],parseCursor:value=>{if(value.scope!==scope||!z.uuid().safeParse(value.relationId).success)fail('invalid_input','关系游标不属于本次查询');return value;}});
      const visible=await readMany(ctx,[...new Set(page.rows.flatMap(row=>[row.fromId,row.toId]))].map(id=>({id,...request}))),visibleIds=new Set(visible.map(item=>item.ref.id));
      const evidence=await sources.resolveMany(ctx,page.rows.flatMap(row=>row.evidenceRefs),request);
      return {items:page.rows.filter(row=>visibleIds.has(row.fromId)&&visibleIds.has(row.toId)&&row.evidenceRefs.every(ref=>{const result=evidence.get(canonicalRefKey(ref));return result&&!(result instanceof DomainError)&&result.known;})).map(({scope:_,...row})=>row),nextCursor:page.hasNextPage?page.endCursor:undefined};
    },
  };
  return operations;
}
