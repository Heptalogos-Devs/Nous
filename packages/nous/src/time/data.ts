// Adapted from Heptalogos time/data.ts; captures are Nous's only time-owned records.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { type Kysely, sql } from 'kysely';
import { TimeCaptureDataRecordSchema } from '@nous-wave/contracts/time';
import { createRecordDataOwner, subjectScope } from '@nous-wave/platform/data';
import { problem } from '@nous-wave/platform/errors';
import type { TimeStore } from './index.js';

export const timeDataMigration = {
  async up(db: Kysely<any>) {
    await sql`create table time_export_members(selection_id uuid not null,ordinal bigint generated always as identity,collection text not null,record jsonb not null,primary key(selection_id,ordinal))`.execute(
      db,
    );
  },
  async down(db: Kysely<any>) {
    await db.schema.dropTable('timeExportMembers').execute();
  },
};

export function createTimeData(store: TimeStore) {
  function validate(record: unknown, subjectId?: string) {
    const row = TimeCaptureDataRecordSchema.parse(record);
    if (subjectId !== undefined && row.subject_id !== subjectId)
      throw problem(
        'time.subject_mismatch',
        'forbidden',
        '归档时间记录不属于该主体',
      );
    const connection = row.connection_ref;
    if (
      connection &&
      (connection.product !== 'nous' ||
        connection.installationId !== store.installationId ||
        connection.kind !== 'ClockSource')
    )
      throw problem(
        'time.connection_scope',
        'conflict',
        '归档ClockSource不属于原安装',
      );
    const capture = row.receipt.captureRef;
    if (
      capture.product === 'nous' &&
      (capture.installationId !== store.installationId || capture.id !== row.id)
    )
      throw problem(
        'time.restore_identity_required',
        'conflict',
        '归档时间回执需要原安装与固定身份',
      );
    return row;
  }
  return createRecordDataOwner(
    {
      product: 'nous',
      installationId: store.installationId,
      get db() {
        return store.db;
      },
      authorize: store.authorize,
    },
    {
      name: 'time',
      collections: [{ table: 'time_captures', scope: subjectScope, validate }],
      beforeDelete: async (ctx, db) => {
        await sql`delete from time_export_members where record->>'subject_id'=${ctx.subjectId}`.execute(
          db,
        );
      },
    },
  );
}
