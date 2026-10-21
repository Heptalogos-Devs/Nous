// Prompt versions and frozen render closures are restored by their owner, without re-rendering history.
import { z } from 'zod';
import { sql,type Kysely } from 'kysely';
import { join } from 'node:path';
import { rm } from 'node:fs/promises';
import { ActorSchema,InstantInputSchema,JsonValueSchema,SubjectIdSchema,type CallContext } from '@nous-wave/contracts/common';
import { PromptSnapshotSchema } from '@nous-wave/contracts/prompts';
import { createRecordDataOwner,type RecordStore } from '../data/records.js';
import { problem } from '../errors.js';
import {digest} from '../settings/catalog.js';
import {checkText} from './registry.js';
const revision=z.string().regex(/^\d+$/),uuid=z.uuid();
export function createPromptDataLifecycle(store:RecordStore,options:{overrideRoot:string}){
 const subject=(id:string)=>sql<boolean>`t.scope_kind='subject' and t.scope_id=${id}`;
 const owner=createRecordDataOwner(store,{name:'prompts',collections:[
  {table:'prompt_revisions',scope:subject,stringColumns:['revision'],validate(row,id){const value=z.strictObject({id:uuid,prompt_id:z.string(),locale:z.string(),scope_kind:z.literal('subject'),scope_id:z.literal(id),revision,source_text:z.string(),digest:z.string(),params_schema_revision:z.string(),dependencies:JsonValueSchema,origin:JsonValueSchema,actor:ActorSchema,reason:z.string().nullable(),recorded_at:InstantInputSchema}).parse(row);if(checkText(value.source_text,64*1024*1024)!==value.digest)throw problem('prompts.data_digest','validation','Prompt修订正文与摘要不符');}},
  {table:'prompt_heads',scope:subject,stringColumns:['revision','latest_revision','active_revision'],validate(row,id){z.strictObject({prompt_id:z.string(),locale:z.string(),scope_kind:z.literal('subject'),scope_id:z.literal(id),revision,latest_revision:revision,active_revision:revision.nullable()}).parse(row);}},
  {table:'prompt_snapshots',scope:id=>sql<boolean>`t.manifest->'snapshot'->'scope'->>'subjectId'=${id}`,validate(row,id){const value=z.strictObject({id:uuid,digest:z.string(),manifest:z.strictObject({snapshot:PromptSnapshotSchema,sources:z.array(z.strictObject({promptId:z.string(),locale:z.string(),scope:z.strictObject({}).passthrough(),source:z.enum(['program','override']),digest:z.string(),text:z.string(),revision:z.number().optional()})),limits:z.strictObject({sourceBytes:z.number(),closureBytes:z.number(),outputBytes:z.number(),closureCount:z.number(),renderMs:z.number(),memoryBytes:z.number()}),paramsSchemaDigest:z.string()}).passthrough(),recorded_at:InstantInputSchema}).parse(row);if(value.manifest.snapshot.scope.kind!=='subject'||value.manifest.snapshot.scope.subjectId!==id||value.manifest.snapshot.ref.id!==value.id||value.manifest.snapshot.ref.digest!==value.digest)throw problem('prompts.data_identity','validation','Prompt冻结闭包与主体或摘要身份不同');const {ref,...snapshot}=value.manifest.snapshot;for(const source of value.manifest.sources)if(checkText(source.text,value.manifest.limits.sourceBytes)!==source.digest)throw problem('prompts.data_digest','validation','Prompt闭包来源正文与摘要不符');if(digest({snapshot,sources:value.manifest.sources,limits:value.manifest.limits,paramsSchemaDigest:value.manifest.paramsSchemaDigest})!==ref.digest)throw problem('prompts.data_digest','validation','Prompt冻结闭包摘要不符');}},
  {table:'prompt_change_receipts',scope:subject,validate(row,id){z.strictObject({operation_id:z.string(),action:z.string(),prompt_id:z.string(),locale:z.string(),scope_kind:z.literal('subject'),scope_id:z.literal(id),request_digest:z.string(),result:JsonValueSchema,actor_authority_ref:z.string(),consumer:JsonValueSchema,recorded_at:InstantInputSchema}).parse(row);}},
 ]});
 return {...owner,async cleanupSubject(subjectId:string){SubjectIdSchema.parse(subjectId);await rm(join(options.overrideRoot,'subject',encodeURIComponent(subjectId)),{recursive:true,force:true});}};
}
export const promptDataMigration={async up(db:Kysely<unknown>){await sql`create table prompts_export_members(selection_id uuid not null,ordinal bigint generated always as identity,collection text not null,record jsonb not null,primary key(selection_id,ordinal))`.execute(db);},async down(db:Kysely<unknown>){await sql`drop table prompts_export_members`.execute(db);}};
