import { sql, type Kysely } from 'kysely';
export async function up(db: Kysely<unknown>) {
  await sql`
    CREATE TABLE subjects(id uuid PRIMARY KEY,display_name text NOT NULL,capabilities jsonb NOT NULL,status text NOT NULL,change_cursor bigint NOT NULL DEFAULT 0,created_at timestamptz NOT NULL,branched_from jsonb);
    CREATE TABLE subject_bindings(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),installation_id text NOT NULL,remote_subject_id uuid NOT NULL,connection_ref text NOT NULL,recorded_at timestamptz NOT NULL,UNIQUE(subject_id,installation_id));
    CREATE TABLE readable_refs(subject_id uuid NOT NULL REFERENCES subjects(id),alias text NOT NULL,object_id uuid NOT NULL,kind text NOT NULL,PRIMARY KEY(subject_id,alias),UNIQUE(subject_id,object_id));
    CREATE TABLE materials(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),current_version int NOT NULL,policy jsonb NOT NULL,status text NOT NULL,created_at timestamptz NOT NULL);
    CREATE TABLE material_versions(id uuid PRIMARY KEY,material_id uuid NOT NULL REFERENCES materials(id) ON DELETE CASCADE,version int NOT NULL,body jsonb NOT NULL,recorded_at timestamptz NOT NULL,UNIQUE(material_id,version));
    CREATE TABLE blobs(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),digest text NOT NULL,bytes bigint NOT NULL,media_type text NOT NULL,recorded_at timestamptz NOT NULL);
    CREATE TABLE encounters(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),step_key text NOT NULL,input_hash text NOT NULL,body jsonb NOT NULL,recorded_at timestamptz NOT NULL,UNIQUE(subject_id,step_key));
    CREATE TABLE experience_items(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),encounter_id uuid NOT NULL UNIQUE REFERENCES encounters(id),body jsonb NOT NULL,observed_at timestamptz NOT NULL,clock_id text NOT NULL,recorded_at timestamptz NOT NULL,processed boolean NOT NULL DEFAULT false);
    CREATE INDEX experience_time ON experience_items(subject_id,clock_id,observed_at);
    CREATE TABLE knowledge_objects(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),kind text NOT NULL,current_revision int NOT NULL,status text NOT NULL,policy jsonb NOT NULL,alias text NOT NULL,created_at timestamptz NOT NULL,UNIQUE(subject_id,alias));
    CREATE INDEX knowledge_subject_kind ON knowledge_objects(subject_id,kind,status);
    CREATE TABLE knowledge_revisions(id uuid PRIMARY KEY,object_id uuid NOT NULL REFERENCES knowledge_objects(id) ON DELETE CASCADE,revision int NOT NULL,body jsonb NOT NULL,source_refs jsonb NOT NULL,source_roots jsonb NOT NULL,formed jsonb NOT NULL,valid jsonb,recorded_at timestamptz NOT NULL,formation text NOT NULL,stance text NOT NULL,confidence double precision NOT NULL,actor jsonb NOT NULL,reason text,UNIQUE(object_id,revision));
    CREATE INDEX knowledge_formed ON knowledge_revisions(object_id,((formed->>'clockId')),((formed->>'at')));
    CREATE TABLE source_links(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),from_ref jsonb NOT NULL,to_ref jsonb NOT NULL,relation text NOT NULL,roots jsonb NOT NULL);
    CREATE INDEX source_target ON source_links USING gin(to_ref);
    CREATE TABLE object_relations(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),from_id uuid NOT NULL,to_id uuid NOT NULL,relation text NOT NULL,weight double precision NOT NULL,evidence_refs jsonb NOT NULL,recorded_at timestamptz NOT NULL);
    CREATE INDEX relation_from ON object_relations(subject_id,from_id);
    CREATE INDEX relation_to ON object_relations(subject_id,to_id);
    CREATE TABLE work_contexts(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),revision int NOT NULL,body jsonb NOT NULL,recorded_at timestamptz NOT NULL);
    CREATE TABLE context_views(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),body jsonb NOT NULL,recorded_at timestamptz NOT NULL);
    CREATE TABLE formation_artifacts(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),body jsonb NOT NULL,recorded_at timestamptz NOT NULL);
    CREATE TABLE use_events(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),step_key text NOT NULL,kind text NOT NULL,refs jsonb NOT NULL,basis_refs jsonb NOT NULL DEFAULT '[]',recorded_at timestamptz NOT NULL,UNIQUE(subject_id,step_key,kind));
    CREATE TABLE resources(id uuid PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),revision int NOT NULL,body jsonb NOT NULL,recorded_at timestamptz NOT NULL);
    CREATE TABLE resource_versions(id uuid PRIMARY KEY,resource_id uuid NOT NULL REFERENCES resources(id),version text NOT NULL,material_ref jsonb NOT NULL,recorded_at timestamptz NOT NULL,UNIQUE(resource_id,version));
    CREATE TABLE subject_changes(subject_id uuid NOT NULL REFERENCES subjects(id),cursor bigint NOT NULL,changed_refs jsonb NOT NULL,kinds jsonb NOT NULL,recorded_at timestamptz NOT NULL,PRIMARY KEY(subject_id,cursor));
    CREATE TABLE settings_overrides(scope text NOT NULL,scope_id text NOT NULL,revision int NOT NULL,body jsonb NOT NULL,recorded_at timestamptz NOT NULL,PRIMARY KEY(scope,scope_id));
    CREATE TABLE settings_history(id uuid PRIMARY KEY,scope text NOT NULL,scope_id text NOT NULL,revision int NOT NULL,body jsonb NOT NULL,actor jsonb NOT NULL,recorded_at timestamptz NOT NULL,UNIQUE(scope,scope_id,revision));
    CREATE TABLE model_resources(id text PRIMARY KEY,body jsonb NOT NULL,recorded_at timestamptz NOT NULL);
    CREATE TABLE model_invocations(id text PRIMARY KEY,subject_id uuid NOT NULL REFERENCES subjects(id),role text NOT NULL,model_resource_id text NOT NULL,status text NOT NULL,reserved_cost double precision NOT NULL,actual_cost double precision,usage jsonb NOT NULL,provider_request_id text,input_refs jsonb NOT NULL,workflow_id text NOT NULL,trace_id text NOT NULL,started_at timestamptz NOT NULL,completed_at timestamptz);
    CREATE INDEX invocation_costs ON model_invocations(subject_id,role,started_at);
  `.execute(db);
}
export async function down() { throw new Error('Use a fresh database for a development rebuild.'); }
