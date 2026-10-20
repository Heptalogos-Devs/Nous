import { sql,type Kysely } from 'kysely';
export async function up(db:Kysely<unknown>) {
  await sql`ALTER TABLE model_invocations ADD COLUMN request jsonb NOT NULL DEFAULT '{}', ADD COLUMN request_hash text NOT NULL DEFAULT '', ADD COLUMN completion_hash text`.execute(db);
}
export async function down(){throw new Error('Model invocation records preserve external effects.');}
