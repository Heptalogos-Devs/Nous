import {requireSubject} from '../platform/security.js';
import {parseInstant} from '@nous-wave/platform/time';
import { sql } from 'kysely';
import { z } from 'zod';
import type { CallContext, Consumer, Policy, Ref, Selector, TimePoint, TimeRange } from '@nous-wave/contracts/common';
import { MaterialSourceSchema } from '@nous-wave/contracts/materials';
import { fail } from '../platform/errors.js';
import { reference, type Storage } from '../platform/context.js';

export interface SourceSnapshot {
  requested: Ref;
  ref: Ref;
  scope?:Consumer;
  inputs: Ref[];
  roots: Ref[];
  policy: Policy;
  recorded: TimePoint;
  observed?: TimePoint;
  consumerKind?: string;
  textLength?: number;
  mediaType?: string;
  occurred?: TimeRange;
}
const all: Policy = { readers: ['*'], purposes: ['*'], audiences: ['*'] };

export function normalizedSelector(selector: Selector | undefined, textLength?: number, mediaType?: string): Selector | undefined {
  if (!selector) return undefined;
  if (selector.kind === 'text') {
    if (textLength === undefined || selector.within) fail('capability_unavailable', '来源没有所选的纯文本区域');
    const start = selector.start ?? 0, end = selector.end ?? textLength;
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > textLength) fail('invalid_input', '来源文本区域超出 Unicode code point 范围');
    return { ...selector, start, end };
  }
  if (selector.kind === 'image') {
    if (!mediaType?.startsWith('image/')) fail('capability_unavailable', '来源不是图像');
    const x = selector.x ?? 0, y = selector.y ?? 0, width = selector.width ?? 1, height = selector.height ?? 1;
    if (x + width > 1 || y + height > 1) fail('invalid_input', '图像区域超出归一化边界');
    return { ...selector, x, y, width, height };
  }
  if (selector.kind === 'media' && selector.end !== undefined && selector.end < (selector.start ?? 0)) fail('invalid_input', '音视频区域的结束早于开始');
  return selector;
}

export function createSourceRepository(store: Storage) {
  async function snapshots(ctx: CallContext, refs: Ref[],db=store.db): Promise<SourceSnapshot[]> {requireSubject(ctx);
    if (!refs.length) return [];
    for (const ref of refs) if (!z.uuid().safeParse(ref.id).success) fail('not_found', '来源身份不存在');
    const rows = await sql<{
      requested: Ref; scope:Consumer|null; id: string; revision: number | null; kind: string; policy: Policy | null;
      source: unknown; inputs: Ref[]; observed: TimePoint | null; consumerKind: string | null;
      recordedAt: string; textLength: number | null; mediaType: string | null; occurred: TimeRange | null;
    }>`with requested as (select value as ref from jsonb_array_elements(${JSON.stringify(refs)}::jsonb))
      select q.ref as requested,v.body->'scope' as scope,m.id,v.version as revision,'Material'::text as kind,m.policy,
        v.body->'source' as source,'[]'::jsonb as inputs,null::jsonb as observed,null::text as consumer_kind,
        v.recorded_at,char_length(v.body->>'text') as text_length,v.body->>'mediaType' as media_type,v.body->'occurred' as occurred
      from requested q join materials m on m.id=(q.ref->>'id')::uuid and m.subject_id=${ctx.subjectId}::uuid
      join material_versions v on v.material_id=m.id and v.version=coalesce((q.ref->>'revision')::integer,m.current_version)
      where q.ref->>'kind'='Material' and m.status<>'purged'
      union all
      select q.ref,e.body->'consumer',e.id,null,'Encounter',null::jsonb,null::jsonb,jsonb_path_query_array(e.body,'$.portions[*].ref'),
        e.body->'observed',e.body->'consumer'->>'kind',e.recorded_at,null,null,null
      from requested q join encounters e on e.id=(q.ref->>'id')::uuid and e.subject_id=${ctx.subjectId}::uuid
      where q.ref->>'kind'='Encounter'
      union all
      select q.ref,c.body->'consumer',e.id,null,'ExperienceItem',null::jsonb,null::jsonb,jsonb_path_query_array(e.body,'$.portions[*].ref'),
        e.body->'observed','subject',e.recorded_at,null,null,null
      from requested q join experience_items e on e.id=(q.ref->>'id')::uuid and e.subject_id=${ctx.subjectId}::uuid join encounters c on c.id=e.encounter_id
      where q.ref->>'kind'='ExperienceItem'`.execute(db);
    return rows.rows.map(row => {
      if (row.kind !== 'Material' && (row.requested.selector || row.requested.revision)) fail('invalid_input', '不可变接触与经历不接受修订或区域');
      const source = row.kind === 'Material' ? MaterialSourceSchema.parse(row.source) : undefined;
      const selector = normalizedSelector(row.requested.selector, row.textLength ?? undefined, row.mediaType ?? undefined);
      return { requested: row.requested,scope:row.scope??undefined, ref: { ...reference(store.installationId, ctx.subjectId, row.kind, row.id, row.revision ?? undefined), selector },
        inputs: source?.inputs ?? row.inputs, roots: source?.roots ?? [], policy: row.policy ?? all,
        recorded: { domainId: 'real', at: parseInstant(row.recordedAt) }, observed: row.observed ?? undefined,
        consumerKind: row.consumerKind ?? undefined, textLength: row.textLength ?? undefined, mediaType: row.mediaType ?? undefined, occurred: row.occurred ?? undefined };
    });
  }
  async function observations(ctx: CallContext, ids: string[], at: TimePoint, recordedAsOf?: string,db=store.db) {requireSubject(ctx);
    if (!ids.length) return [];
    return (await sql<{ ref: Ref; observed: TimePoint }>`select l.to_ref as ref,e.body->'observed' as observed
      from source_links l join experience_items e on e.id=(l.from_ref->>'id')::uuid and e.subject_id=l.subject_id
      where l.subject_id=${ctx.subjectId}::uuid and l.from_ref->>'kind'='ExperienceItem'
      and l.to_ref->>'id'=any(${ids}::text[]) and e.domain_id=${at.domainId} and e.observed_at<=${at.at}::timestamptz
      and (${recordedAsOf ?? null}::timestamptz is null or e.recorded_at<=${recordedAsOf ?? null}::timestamptz)`.execute(db)).rows;
  }
  return { snapshots, observations };
}
