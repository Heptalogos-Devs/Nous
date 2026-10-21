import {requireSubject} from '../platform/security.js';
import { sql, type Selectable } from 'kysely';
import type { z } from 'zod';
import type { CallContext } from '@nous-wave/contracts/common';
import type { KnowledgeReadSchema, KnowledgeListSchema } from '@nous-wave/contracts/knowledge';
import type { Database } from '../platform/database.js';
import type { Storage } from '../platform/context.js';

export interface KnowledgeRecord { object: Selectable<Database['knowledgeObjects']>; revision: Selectable<Database['knowledgeRevisions']> }
export type ReadRequest = z.input<typeof KnowledgeReadSchema>;
export type ListRequest = z.infer<typeof KnowledgeListSchema>;

export function createKnowledgeRepository(store: Storage) {
  async function records(ctx: CallContext, requests: ReadRequest[],db=store.db): Promise<Array<KnowledgeRecord & { requested: ReadRequest }>> {requireSubject(ctx);
    if (!requests.length) return [];
    return (await sql<KnowledgeRecord & { requested: ReadRequest }>`with requested as(select value as request,ordinality as position from jsonb_array_elements(${JSON.stringify(requests)}::jsonb) with ordinality)
      select q.request as requested,
        jsonb_build_object('id',o.id,'subjectId',o.subject_id,'kind',o.kind,'currentRevision',o.current_revision,'status',o.status,'policy',o.policy,'alias',o.alias,'createdAt',o.created_at) as object,
        jsonb_build_object('id',r.id,'objectId',r.object_id,'revision',r.revision,'body',r.body,'sourceRefs',r.source_refs,'sourceRoots',r.source_roots,'formed',r.formed,'valid',r.valid,'recordedAt',r.recorded_at,'formation',r.formation,'stance',r.stance,'confidence',r.confidence,'actor',r.actor,'reason',r.reason) as revision
      from requested q join knowledge_objects o on(o.id=case when q.request->>'id'~*'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' then(q.request->>'id')::uuid end or o.alias=q.request->>'id') and o.subject_id=${ctx.subjectId}::uuid and o.status<>'purged'
      join lateral(select * from knowledge_revisions v where v.object_id=o.id
        and(q.request->>'revision' is null or v.revision=(q.request->>'revision')::integer)
        and(q.request->>'recordedAsOf' is null or v.recorded_at<=(q.request->>'recordedAsOf')::timestamptz)
        and(q.request->'knowledgeAt' is null or(v.formed->>'domainId'=q.request->'knowledgeAt'->>'domainId' and v.formed->>'at'<=q.request->'knowledgeAt'->>'at'))
        order by v.revision desc limit 1) r on true order by q.position`.execute(db)).rows;
  }
  function candidates(ctx: CallContext, input: ListRequest, scope: string,db=store.db) {requireSubject(ctx);
    let query = db.selectFrom('knowledgeObjects as o').innerJoinLateral(eb => {
      let revision = eb.selectFrom('knowledgeRevisions as r').select(['r.objectId','r.revision','r.body','r.sourceRefs','r.sourceRoots']).whereRef('r.objectId','=','o.id');
      if (input.knowledgeAt) revision=revision.where(sql<boolean>`r.formed->>'domainId'=${input.knowledgeAt.domainId} and r.formed->>'at'<=${input.knowledgeAt.at}`);
      if (input.recordedAsOf) revision=revision.where('r.recordedAt','<=',input.recordedAsOf);
      const ordered=revision.orderBy('r.revision','desc');return(input.includeHistory?ordered:ordered.limit(1)).as('v');
    }, join=>join.onTrue()).select(['o.id','v.revision']).select(sql<string>`${scope}`.as('scope')).where('o.subjectId','=',ctx.subjectId).where('o.status','!=','purged');
    if (!input.includeInactive) query=query.where('o.status','=','active');
    if (input.ids?.length) query=query.where('o.id','in',input.ids);
    if (input.kinds?.length) query=query.where('o.kind','in',input.kinds);
    if (input.conceptIds?.length) query=query.where(eb=>eb.exists(eb.selectFrom('knowledgeConcepts as kc').select('kc.objectId').whereRef('kc.objectId','=','o.id').whereRef('kc.revision','=','v.revision').where('kc.conceptId','in',input.conceptIds!)));
    if(input.range)query=query.where(sql<boolean>`v.body->'range' @> ${JSON.stringify(input.range)}::jsonb`);
    if(input.refs?.length)query=query.where(sql<boolean>`o.id::text=any(${input.refs}::text[]) or o.alias=any(${input.refs}::text[])`);
    if(input.tags?.length){const predicates=input.tags.map(tag=>sql<boolean>`v.body->'tags' @> ${JSON.stringify([tag])}::jsonb`);query=query.where(eb=>input.tagMatch==='all'?eb.and(predicates):eb.or(predicates));}
    if(input.entities?.length){const predicates=input.entities.map(id=>sql<boolean>`exists(select 1 from jsonb_array_elements(v.body->'entityRefs') entity where entity->>'id'=${id} or lower(entity->>'kind')||':'|| (entity->>'id')=${id})`);query=query.where(eb=>input.entityMatch==='all'?eb.and(predicates):eb.or(predicates));}
    if(input.sources?.length)query=query.where(sql<boolean>`exists(select 1 from jsonb_array_elements(v.source_refs||v.source_roots) source where source->>'id'=any(${input.sources}::text[]) or lower(source->>'kind')||':'||(source->>'id')=any(${input.sources}::text[]))`);
    return query;
  }
  return { records, candidates };
}
