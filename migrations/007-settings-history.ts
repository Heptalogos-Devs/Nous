import {sql,type Kysely} from 'kysely';
export async function up(db:Kysely<unknown>) {
  await sql`alter table settings_history add column previous_body jsonb not null default '{}'::jsonb,add column reason text`.execute(db);
  await sql`update settings_history h set previous_body=p.previous from(select id,coalesce(lag(body)over(partition by scope,scope_id order by revision),'{}'::jsonb)as previous from settings_history)p where h.id=p.id`.execute(db);
  await sql`create unique index settings_history_revision on settings_history(scope,scope_id,revision)`.execute(db);
}
export async function down(db:Kysely<unknown>) {
  await sql`drop index settings_history_revision`.execute(db);
  await sql`alter table settings_history drop column previous_body,drop column reason`.execute(db);
}
