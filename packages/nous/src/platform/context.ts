import {requireSubject} from './security.js';
import { randomBytes } from 'node:crypto';
import { v7 as uuid } from 'uuid';
import type { CallContext, Ref, Consumer } from '@nous-wave/contracts/common';
import { sql } from 'kysely';
import type { Kysely,IsolationLevel } from 'kysely';
import type { Database } from './database.js';
import { trace,isSpanContextValid } from '@opentelemetry/api';
export {readReal as now} from '@nous-wave/platform/time';
import {readReal as now} from '@nous-wave/platform/time';
export const json = <T>(value: T) => sql<T>`${JSON.stringify(value)}::jsonb`;
export function makeContext(input:{
  actor:CallContext['actor'];consumer:Consumer;subjectId?:string;purpose?:string;audience?:string[];
  operationId?:string;traceparent?:string;invocationId?:string;taskId?:string;effectId?:string;
}):CallContext {
  const active=trace.getActiveSpan()?.spanContext();
  const current=active&&isSpanContextValid(active)?active:undefined;
  return {
    actor:input.actor,consumer:input.consumer,subjectId:input.subjectId,purpose:input.purpose??'management',audience:input.audience??[],
    trace:{operationId:input.operationId??uuid(),traceId:current?.traceId??input.traceparent?.split('-')[1]??randomBytes(16).toString('hex'),
      traceparent:current?`00-${current.traceId}-${current.spanId}-${current.traceFlags.toString(16).padStart(2,'0')}`:input.traceparent,
      invocationId:input.invocationId,taskId:input.taskId,effectId:input.effectId},
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
  requireSubject(ctx);
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
  transaction<T>(fn: (db: Kysely<Database>) => Promise<T>,options?:{isolationLevel?:IsolationLevel}): Promise<T>;
  installationId: string;
  authorize(ctx:CallContext,operation:string,target?:Ref,db?:Kysely<any>):Promise<void>;
}
