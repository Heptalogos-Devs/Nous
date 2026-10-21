import { sql, type Kysely } from 'kysely';

export async function up(db: Kysely<unknown>) {
  await sql`alter table blobs add column submission_key text`.execute(db);
  await sql`update blobs set submission_key = id::text`.execute(db);
  await sql`alter table blobs alter column submission_key set not null`.execute(db);
  await sql`create unique index blobs_submission on blobs(subject_id, submission_key)`.execute(db);
  await sql`create index material_versions_blob on material_versions ((body->'blobRef'->>'id')) where body ? 'blobRef'`.execute(db);
}
export async function down(db: Kysely<unknown>) {
  await sql`drop index material_versions_blob`.execute(db);
  await sql`drop index blobs_submission`.execute(db);
  await sql`alter table blobs drop column submission_key`.execute(db);
}
