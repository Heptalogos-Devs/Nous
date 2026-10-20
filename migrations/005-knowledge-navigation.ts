import { sql, type Kysely } from 'kysely';

export async function up(db: Kysely<unknown>) {
  await sql`create index formation_artifacts_experiences on formation_artifacts using gin((body->'experienceRefs') jsonb_path_ops)`.execute(db);
  await sql`create index experience_items_collect on experience_items(subject_id,observed_at,id) where processed=false`.execute(db);
  await sql`create table knowledge_concepts (
    subject_id uuid not null references subjects(id), object_id uuid not null references knowledge_objects(id),
    revision integer not null, concept_id uuid not null references knowledge_objects(id),
    primary key(object_id,revision,concept_id), foreign key(object_id,revision) references knowledge_revisions(object_id,revision)
  )`.execute(db);
  await sql`create index knowledge_concepts_members on knowledge_concepts(subject_id,concept_id,object_id,revision)`.execute(db);
  await sql`insert into knowledge_concepts(subject_id,object_id,revision,concept_id)
    select distinct o.subject_id,r.object_id,r.revision,c.id from knowledge_revisions r
    join knowledge_objects o on o.id=r.object_id cross join lateral jsonb_array_elements(coalesce(r.body->'conceptRefs','[]')) p(ref)
    join knowledge_objects c on c.id::text=p.ref->>'id' and c.subject_id=o.subject_id and c.kind='Concept'`.execute(db);
  await sql`delete from source_links a using source_links b where a.subject_id=b.subject_id and a.from_ref=b.from_ref and a.to_ref=b.to_ref and a.relation=b.relation and a.id>b.id`.execute(db);
  await sql`create unique index source_links_edge on source_links(subject_id,from_ref,to_ref,relation)`.execute(db);
  await sql`create unique index knowledge_objects_alias on knowledge_objects(subject_id,alias)`.execute(db);
  await sql`create index source_links_target on source_links(subject_id,(to_ref->>'id'),(from_ref->>'kind'))`.execute(db);
  await sql`create index source_links_origin on source_links(subject_id,(from_ref->>'kind'),(from_ref->>'id'),(coalesce(from_ref->>'revision','0')))`.execute(db);
  await sql`insert into source_links(id,subject_id,from_ref,to_ref,relation,roots)
    select uuidv7(),m.subject_id,v.body->'ref',p.ref,'derived_from',coalesce(v.body->'source'->'roots','[]')
    from material_versions v join materials m on m.id=v.material_id
    cross join lateral jsonb_array_elements(coalesce(v.body->'source'->'inputs','[]')) p(ref) on conflict do nothing`.execute(db);
  await sql`insert into source_links(id,subject_id,from_ref,to_ref,relation,roots)
    select uuidv7(),e.subject_id,jsonb_build_object('product','nous','installationId',
      coalesce(e.body->'contextViewRef'->>'installationId',e.body->'portions'->0->'ref'->>'installationId'),
      'subjectId',e.subject_id,'kind','Encounter','id',e.id),p.value->'ref','derived_from','[]'::jsonb
    from encounters e cross join lateral jsonb_array_elements(e.body->'portions') p(value) on conflict do nothing`.execute(db);
  await sql`insert into source_links(id,subject_id,from_ref,to_ref,relation,roots)
    select uuidv7(),e.subject_id,e.body->'ref',p.value->'ref','derived_from',coalesce(e.body->'sourceRoots','[]')
    from experience_items e cross join lateral jsonb_array_elements(e.body->'portions') p(value) on conflict do nothing`.execute(db);
  await sql`alter table object_relations add column relation_id uuid,add column revision integer,add column formed jsonb,add column valid jsonb,add column roots jsonb not null default '[]'`.execute(db);
  await sql`with versions as (select id,first_value(id) over w as relation_id,row_number() over w as revision
    from object_relations window w as(partition by subject_id,from_id,to_id,relation order by recorded_at,id))
    update object_relations r set relation_id=v.relation_id,revision=v.revision,formed=jsonb_build_object('clockId','real','at',to_char(r.recorded_at at time zone 'UTC','YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')) from versions v where v.id=r.id`.execute(db);
  await sql`alter table object_relations alter column relation_id set not null,alter column revision set not null,alter column formed set not null`.execute(db);
  await sql`create unique index object_relations_revision on object_relations(subject_id,relation_id,revision)`.execute(db);
  await sql`create index object_relations_history on object_relations(subject_id,from_id,to_id,relation,revision desc)`.execute(db);
  await sql`create index knowledge_revisions_cutoff on knowledge_revisions(object_id,(formed->>'clockId'),(formed->>'at'),recorded_at,revision desc)`.execute(db);
}
export async function down(db: Kysely<unknown>) {
  await db.schema.dropTable('knowledgeConcepts').execute();
  await sql`drop index formation_artifacts_experiences;drop index experience_items_collect;drop index knowledge_objects_alias;drop index source_links_target;drop index source_links_edge;drop index source_links_origin;drop index object_relations_revision;drop index object_relations_history;drop index knowledge_revisions_cutoff`.execute(db);
  await sql`alter table object_relations drop column relation_id,drop column revision,drop column formed,drop column valid,drop column roots`.execute(db);
}
