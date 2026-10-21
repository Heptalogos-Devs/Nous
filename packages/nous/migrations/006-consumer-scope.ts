import { sql,type Kysely } from 'kysely';

export async function up(db:Kysely<unknown>) {
  await sql`update material_versions v set body=jsonb_set(v.body,'{scope}',coalesce(
    (select e.body->'consumer' from encounters e where e.subject_id=m.subject_id
      and (e.body->'resultRef'->>'id'=m.id::text or exists(select 1 from jsonb_array_elements(e.body->'portions') p where p->'ref'->>'id'=m.id::text))
      order by e.recorded_at desc limit 1),
    jsonb_build_object('kind','subject','host','nous','id',m.subject_id::text)))
    from materials m where m.id=v.material_id and not(v.body ? 'scope')`.execute(db);
  await sql`update work_contexts w set body=jsonb_set(w.body,'{scope}',coalesce(
    (select v.body->'consumer' from context_views v where v.subject_id=w.subject_id and v.body->>'workContextId'=w.id::text order by v.recorded_at desc limit 1),
    jsonb_build_object('kind','subject','host','nous','id',w.subject_id::text))) where not(w.body ? 'scope')`.execute(db);
}
export async function down(db:Kysely<unknown>) {
  await sql`update material_versions set body=body-'scope'`.execute(db);
  await sql`update work_contexts set body=body-'scope'`.execute(db);
}
