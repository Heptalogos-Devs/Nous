import { randomBytes } from 'node:crypto';
import { v7 as uuid } from 'uuid';
import type { CallContext, Ref, TimePoint } from '@nous-wave/contracts/common';
import { sql } from 'kysely';
import type { Kysely } from 'kysely';
import type { Database } from './database.js';
import { trace,isSpanContextValid } from '@opentelemetry/api';
export const now = (): TimePoint => ({
  clockId: 'real',
  at: new Date().toISOString(),
});
export const json = <T>(value: T) => sql<T>`${JSON.stringify(value)}::jsonb`;
export function makeContext(
  actor: CallContext['actor'],
  subjectId: string,
  purpose = 'management',
  audience = [subjectId],
  operationId = uuid(),
  traceparent?: string,
): CallContext {
  const active=trace.getActiveSpan()?.spanContext();
  const current=active&&isSpanContextValid(active)?active:undefined;
  return {
  actor,
  subjectId,
  consumer:actor.consumer,
  purpose,
  audience,
  traceContext: {
    operationId,
    traceId: current?.traceId??traceparent?.split('-')[1] ?? randomBytes(16).toString('hex'),
    traceparent:current?`00-${current.traceId}-${current.spanId}-${current.traceFlags.toString(16).padStart(2,'0')}`:traceparent,
  },
};
}
export function reference(
  installationId: string,
  subjectId: string,
  kind: string,
  id: string,
  revision?: number,
): Ref {
  return {
    product: 'nous',
    installationId,
    subjectId,
    kind,
    id,
    ...(revision ? { revision } : {}),
  };
}
export async function changed(
  db: Kysely<Database>,
  ctx: CallContext,
  refs: Ref[],
) {
  const subject = await db
    .updateTable('subjects')
    .set({ changeCursor: sql<string>`change_cursor + 1` })
    .where('id', '=', ctx.subjectId)
    .returning('changeCursor')
    .executeTakeFirstOrThrow();
  await db
    .insertInto('subjectChanges')
    .values({
      subjectId: ctx.subjectId,
      cursor: subject.changeCursor,
      changedRefs: json(refs),
      kinds: json([...new Set(refs.map((r) => r.kind))]),
      recordedAt: now().at,
    })
    .execute();
}
export interface Storage {
  readonly db: Kysely<Database>;
  transaction<T>(fn: () => Promise<T>): Promise<T>;
  installationId: string;
}
