import { sql, type Kysely } from 'kysely';

export async function up(db: Kysely<unknown>) {
  await sql`create table episode_members (
    episode_id uuid not null references knowledge_objects(id), revision integer not null,
    experience_id uuid not null references experience_items(id), subject_id uuid not null references subjects(id),
    track text not null check (track in ('life','social','work')),
    primary key (episode_id, revision, experience_id)
  )`.execute(db);
  await sql`create index episode_members_experience on episode_members(subject_id, experience_id, track)`.execute(db);
  await sql`create table motivation_evaluations (
    id uuid primary key, subject_id uuid not null references subjects(id), object_id uuid not null references knowledge_objects(id),
    criterion_revision integer not null, body jsonb not null, recorded_at timestamptz not null
  )`.execute(db);
  await sql`create index motivation_evaluations_object on motivation_evaluations(subject_id, object_id, recorded_at)`.execute(db);
}

export async function down(db: Kysely<unknown>) {
  await db.schema.dropTable('motivationEvaluations').execute();
  await db.schema.dropTable('episodeMembers').execute();
}
