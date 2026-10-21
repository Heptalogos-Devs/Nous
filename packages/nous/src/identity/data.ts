import { z } from 'zod';
import { sql } from 'kysely';
import { PolicySchema, RefSchema, InstantInputSchema as InstantSchema, SubjectIdSchema } from '@nous-wave/contracts/common';
import { KnowledgeKindSchema } from '@nous-wave/contracts/knowledge';
import type { Storage } from '../platform/context.js';
import { createRecordDataOwner, subjectScope } from '@nous-wave/platform/data';
import { problem } from '@nous-wave/platform/errors';
const subject = z.strictObject({ id: SubjectIdSchema, display_name: z.string(), capabilities: z.array(KnowledgeKindSchema), status: z.enum(['active','paused','stopped']), change_cursor: z.string().regex(/^\d+$/), created_at: InstantSchema, branched_from: RefSchema.nullable() });
export function createIdentityData(store: Storage) {
  return createRecordDataOwner(Object.assign(Object.create(store),{product:'nous' as const}), { name: 'identity', collections: [
    { table:'subjects',scope:id=>sql<boolean>`t.id=${id}::uuid`,stringColumns:['change_cursor'],validate(row,id){ if(subject.parse(row).id!==id)throw problem('data.subject_mismatch','validation','主体身份与归档范围不同'); if(row.status==='active')throw problem('data.subject_active','conflict','请先暂停主体再封存或恢复'); } },
    { table:'subject_bindings',scope:subjectScope,validate(row,id){z.strictObject({id:z.uuid(),subject_id:z.literal(id),installation_id:z.string(),remote_subject_id:z.uuid(),connection_ref:z.string(),recorded_at:InstantSchema}).parse(row);} },
    { table:'subject_changes',scope:subjectScope,stringColumns:['cursor'],validate(row,id){z.strictObject({subject_id:z.literal(id),cursor:z.string().regex(/^\d+$/),changed_refs:z.array(RefSchema),kinds:z.array(z.string()),recorded_at:InstantSchema}).parse(row);} },
  ] });
}
