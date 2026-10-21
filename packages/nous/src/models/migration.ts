import { sql, type Kysely } from 'kysely';
import type { Consumer, Ref } from '@nous-wave/contracts/common';
export interface ModelArtifactsDatabase {
  modelArtifacts: {
    invocationId: string;
    direction: 'input' | 'output';
    digest: string;
    bytes: string;
    consumer: Consumer;
    inputRefs: Ref[];
    recordedAt: string;
  };
}
export const modelArtifactsMigration = {
  async up(db: Kysely<any>) {
    await sql`create table model_artifacts(invocation_id uuid not null references model_invocations(id),direction text not null check(direction in ('input','output')),digest text not null,bytes bigint not null check(bytes>=0),consumer jsonb not null,input_refs jsonb not null,recorded_at timestamptz(6) not null,primary key(invocation_id,direction))`.execute(
      db,
    );
  },
  async down(db: Kysely<any>) {
    await db.schema.dropTable('modelArtifacts').execute();
  },
};
