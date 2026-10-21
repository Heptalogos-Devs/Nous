import type { Actor, Consumer, Ref } from '@nous-wave/contracts/common';
import type { TimeCaptureReceipt } from '@nous-wave/contracts/time';
import type { SettingsDatabase } from '@nous-wave/platform/settings';
import { type ColumnType, type Kysely, sql } from 'kysely';
export interface TimeDatabase extends SettingsDatabase {
  timeCaptures: {
    id: string;
    operationId: string;
    requestDigest: string;
    captureKey: string;
    connectionRef: ColumnType<
      Ref | null,
      string | Ref | null,
      string | Ref | null
    >;
    subjectId: string | null;
    consumer: Consumer;
    actor: Actor;
    originRef: ColumnType<Ref | null, string | Ref | null, string | Ref | null>;
    receipt: ColumnType<
      TimeCaptureReceipt,
      string | TimeCaptureReceipt,
      string | TimeCaptureReceipt
    >;
    recordedAt: string;
  };
}
export const timeMigration = {
  async up(db: Kysely<unknown>) {
    await sql`create table time_captures(id uuid primary key,operation_id text not null unique,request_digest text not null,capture_key text not null,connection_ref jsonb,subject_id uuid,consumer jsonb not null,actor jsonb not null,origin_ref jsonb,receipt jsonb not null,recorded_at timestamptz(6) not null);create index time_capture_subject on time_captures(subject_id);create index time_capture_origin on time_captures(capture_key)`.execute(
      db,
    );
  },
  async down(db: Kysely<unknown>) {
    await db.schema.dropTable('timeCaptures').execute();
  },
};
