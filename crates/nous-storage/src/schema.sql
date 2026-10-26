CREATE TABLE IF NOT EXISTS authority_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    commit_seq INTEGER NOT NULL CHECK (commit_seq >= 0)
);


CREATE TABLE IF NOT EXISTS namespaces (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0)
);


CREATE TABLE IF NOT EXISTS subjects (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL REFERENCES namespaces(id),
    name TEXT NOT NULL,
    capabilities TEXT NOT NULL CHECK (json_valid(capabilities)),
    revision INTEGER NOT NULL CHECK (revision > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    UNIQUE(namespace_id, id)
);
CREATE INDEX IF NOT EXISTS subjects_by_namespace ON subjects(namespace_id, commit_seq, id);

CREATE TABLE IF NOT EXISTS configuration_changes (
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('instance','namespace','subject')),
    namespace_id TEXT REFERENCES namespaces(id),
    subject_id TEXT,
    key TEXT NOT NULL CHECK (length(key) > 0),
    revision INTEGER NOT NULL CHECK (revision > 0),
    active TEXT CHECK (active IS NULL OR json_valid(active)),
    desired TEXT CHECK (desired IS NULL OR json_valid(desired)),
    apply_mode TEXT NOT NULL CHECK (apply_mode IN ('live','restart')),
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq = revision),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id),
    CHECK ((scope_kind='instance' AND namespace_id IS NULL AND subject_id IS NULL)
        OR (scope_kind='namespace' AND namespace_id IS NOT NULL AND subject_id IS NULL)
        OR (scope_kind='subject' AND namespace_id IS NOT NULL AND subject_id IS NOT NULL)),
    CHECK (apply_mode <> 'live' OR active IS desired)
);
CREATE UNIQUE INDEX IF NOT EXISTS configuration_by_scope_key_revision
ON configuration_changes(scope_kind,coalesce(namespace_id,''),coalesce(subject_id,''),key,commit_seq DESC);
CREATE INDEX IF NOT EXISTS configuration_by_commit ON configuration_changes(commit_seq);

CREATE TABLE IF NOT EXISTS prompt_revisions (
    scope TEXT NOT NULL CHECK(json_valid(scope)),
    id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    locale TEXT,
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB)) BETWEEN 1 AND 131072),
    author TEXT NOT NULL,
    reason TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY(scope,id,revision)
);

CREATE TABLE IF NOT EXISTS access_grants (
    id TEXT PRIMARY KEY NOT NULL,
    current_revision INTEGER NOT NULL CHECK(current_revision>0),
    FOREIGN KEY(id,current_revision) REFERENCES grant_revisions(grant_id,revision) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE IF NOT EXISTS grant_revisions (
    grant_id TEXT NOT NULL REFERENCES access_grants(id),
    revision INTEGER NOT NULL CHECK(revision>0),
    kind TEXT NOT NULL CHECK(kind IN('bootstrap','issued')),
    name TEXT NOT NULL,
    caller_id TEXT NOT NULL,
    scope_kind TEXT NOT NULL CHECK(scope_kind IN('admin','namespace','subject')),
    namespace_id TEXT REFERENCES namespaces(id),
    subject_id TEXT,
    parent_id TEXT,
    parent_revision INTEGER,
    can_delegate INTEGER NOT NULL CHECK(can_delegate IN(0,1)),
    expires_at_unix_ms INTEGER CHECK(expires_at_unix_ms>=0),
    state TEXT NOT NULL CHECK(state IN('active','revoked')),
    credential_hash TEXT NOT NULL CHECK(length(credential_hash)=64 AND credential_hash NOT GLOB '*[^0-9a-f]*'),
    secret_ref TEXT NOT NULL CHECK(json_valid(secret_ref)),
    created_at TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK(commit_seq=revision),
    PRIMARY KEY(grant_id,revision),
    FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
    FOREIGN KEY(parent_id,parent_revision) REFERENCES grant_revisions(grant_id,revision),
    CHECK((parent_id IS NULL)=(parent_revision IS NULL)),
    CHECK(parent_id IS NULL OR parent_id<>grant_id),
    CHECK((scope_kind='admin' AND namespace_id IS NULL AND subject_id IS NULL)
        OR(scope_kind='namespace' AND namespace_id IS NOT NULL AND subject_id IS NULL)
        OR(scope_kind='subject' AND namespace_id IS NOT NULL AND subject_id IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS grant_history_by_scope ON grant_revisions(scope_kind,namespace_id,subject_id,commit_seq);
CREATE INDEX IF NOT EXISTS grant_secret_references ON grant_revisions(secret_ref);

CREATE TABLE IF NOT EXISTS materials (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL REFERENCES namespaces(id),
    content_hash TEXT NOT NULL,
    media_type TEXT NOT NULL,
    metadata TEXT NOT NULL CHECK(json_valid(metadata)),
    byte_len INTEGER NOT NULL CHECK (byte_len >= 0),
    source TEXT,
    event_time_domain TEXT,
    event_time_coordinate INTEGER,
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    UNIQUE(namespace_id, id),
    CHECK ((event_time_domain IS NULL) = (event_time_coordinate IS NULL))
);
CREATE INDEX IF NOT EXISTS materials_by_namespace ON materials(namespace_id, commit_seq, id);

CREATE TABLE IF NOT EXISTS observations (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL REFERENCES namespaces(id),
    subject_id TEXT NOT NULL,
    material_id TEXT NOT NULL,
    observed_at_domain TEXT,
    observed_at_coordinate INTEGER,
    origin TEXT CHECK(origin IS NULL OR (json_valid(origin) AND json_type(origin)='object'
        AND (coalesce(json_type(origin,'$.conversation_ref')='text',0) OR coalesce(json_type(origin,'$.external_object_ref')='text',0)))),
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0), kind TEXT NOT NULL CHECK (kind IN ('contact','import')),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id),
    FOREIGN KEY(namespace_id, material_id) REFERENCES materials(namespace_id, id),
    CHECK ((observed_at_domain IS NULL) = (observed_at_coordinate IS NULL))
);
CREATE INDEX IF NOT EXISTS observations_by_subject ON observations(namespace_id, subject_id, commit_seq, id);
CREATE INDEX IF NOT EXISTS observations_origin ON observations(namespace_id,subject_id,json_extract(origin,'$.conversation_ref'),material_id);
CREATE UNIQUE INDEX IF NOT EXISTS observations_scoped_source
ON observations(namespace_id, subject_id, id, material_id);

CREATE TABLE IF NOT EXISTS operation_receipts (
    caller_id TEXT NOT NULL,
    authorization TEXT CHECK(authorization IS NULL OR json_valid(authorization)),
    trace_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    scope TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    commit_seq INTEGER NOT NULL UNIQUE,
    receipt TEXT NOT NULL CHECK (json_valid(receipt)),
    PRIMARY KEY(caller_id, operation, scope, operation_id)
);
CREATE INDEX IF NOT EXISTS operations_by_trace ON operation_receipts(trace_id,commit_seq);


CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    current_revision INTEGER NOT NULL CHECK (current_revision > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    UNIQUE(namespace_id, subject_id, id),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id),
    FOREIGN KEY(id, current_revision) REFERENCES memory_revisions(memory_id, revision)
        DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX IF NOT EXISTS memories_by_subject ON memories(namespace_id, subject_id, commit_seq, id);
CREATE INDEX IF NOT EXISTS maintenance_by_identity ON memories(namespace_id, subject_id, id);

CREATE TABLE IF NOT EXISTS formation_memories (
    job_id TEXT NOT NULL REFERENCES formation_runs(job_id),
    memory_id TEXT NOT NULL REFERENCES memories(id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY(job_id, memory_id),
    UNIQUE(job_id, ordinal)
);


CREATE TABLE IF NOT EXISTS memory_revisions (
    memory_id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    epistemic_status TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    material_id TEXT NOT NULL,
    source TEXT NOT NULL CHECK(json_valid(source) AND json_type(source)='object'),
    model_call_id TEXT,
    times TEXT NOT NULL CHECK (json_valid(times)),
    recorded_at TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0,1)),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    lifecycle_strength REAL NOT NULL CHECK (lifecycle_strength >= 0.0 AND lifecycle_strength <= 1.0),
    lifecycle_anchor_unix_ms INTEGER NOT NULL,
    lifecycle_activation TEXT NOT NULL CHECK (lifecycle_activation IN ('active','frozen','forgotten','decayed','weakened')),
    lifecycle_pinned INTEGER NOT NULL CHECK (lifecycle_pinned IN (0,1)),
    lifecycle_protected_until_unix_ms INTEGER,
    lifecycle_inactive_since_unix_ms INTEGER,
    lifecycle_last_reinforced_unix_ms INTEGER,
    origin TEXT NOT NULL CHECK (origin IN ('model','imported','manual')),
    CHECK ((origin='model' AND model_call_id IS NOT NULL)
        OR (origin IN ('imported','manual') AND model_call_id IS NULL)),
    PRIMARY KEY(memory_id, revision),
    FOREIGN KEY(namespace_id, subject_id, memory_id) REFERENCES memories(namespace_id, subject_id, id),
    FOREIGN KEY(namespace_id, subject_id, observation_id, material_id) REFERENCES observations(namespace_id, subject_id, id, material_id),
    FOREIGN KEY(namespace_id, subject_id, model_call_id) REFERENCES model_calls(namespace_id, subject_id, id)
);
CREATE INDEX IF NOT EXISTS memory_changes ON memory_revisions(commit_seq, memory_id, revision);
CREATE INDEX IF NOT EXISTS subject_memory_changes ON memory_revisions(namespace_id, subject_id, commit_seq, memory_id);

CREATE TABLE IF NOT EXISTS memory_audit (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    action TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason TEXT NOT NULL,
    previous_revision INTEGER NOT NULL CHECK (previous_revision >= 0),
    revision INTEGER NOT NULL CHECK (revision > previous_revision),
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    details TEXT NOT NULL CHECK (json_valid(details)),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id)
);
CREATE INDEX IF NOT EXISTS audit_by_memory ON memory_audit(namespace_id, memory_id, commit_seq, id);

CREATE TABLE IF NOT EXISTS memory_purges (
    memory_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id)
);
CREATE INDEX IF NOT EXISTS purge_changes ON memory_purges(commit_seq, memory_id);
CREATE INDEX IF NOT EXISTS subject_purge_changes ON memory_purges(namespace_id, subject_id, commit_seq, memory_id);

CREATE TABLE IF NOT EXISTS material_details (
    material_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    source_type TEXT,
    tags TEXT NOT NULL CHECK (json_valid(tags)),
    metadata TEXT NOT NULL CHECK (json_valid(metadata)),
    structured TEXT CHECK (structured IS NULL OR json_valid(structured)),
    FOREIGN KEY(namespace_id, material_id) REFERENCES materials(namespace_id, id)
);


CREATE TABLE IF NOT EXISTS model_calls (
    id TEXT PRIMARY KEY NOT NULL,
    trace_id TEXT NOT NULL,
    reservation_commit_seq INTEGER NOT NULL CHECK(reservation_commit_seq > 0),
    job_id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    step_key TEXT NOT NULL,
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    resource_id TEXT NOT NULL,
    request TEXT NOT NULL CHECK (json_valid(request)),
    response TEXT CHECK (response IS NULL OR json_valid(response)),
    state TEXT NOT NULL CHECK (state IN ('started','succeeded','failed','uncertain')),
    error TEXT CHECK (error IS NULL OR json_valid(error)),
    usage TEXT CHECK (usage IS NULL OR json_valid(usage)),
    cost TEXT CHECK (cost IS NULL OR json_valid(cost)),
    created_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(job_id, step_key, attempt),
    UNIQUE(job_id, id),
    UNIQUE(namespace_id, subject_id, id),
    FOREIGN KEY(job_id, namespace_id, subject_id, trace_id)
        REFERENCES task_records(job_id, namespace_id, subject_id, trace_id)
);

CREATE TABLE IF NOT EXISTS model_budget_charges (
    call_id TEXT NOT NULL REFERENCES model_calls(id),
    bucket TEXT NOT NULL CHECK (json_valid(bucket)),
    day TEXT NOT NULL,
    max_calls_per_day INTEGER NOT NULL CHECK (max_calls_per_day BETWEEN 0 AND 100000),
    PRIMARY KEY(call_id,bucket)
);
CREATE INDEX IF NOT EXISTS model_budget_daily ON model_budget_charges(bucket,day);

CREATE TABLE IF NOT EXISTS embedding_spaces (
    id TEXT PRIMARY KEY NOT NULL,
    provider_model TEXT NOT NULL,
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    normalization TEXT NOT NULL CHECK(normalization IN('none','l2')),
    parameters TEXT NOT NULL CHECK(json_valid(parameters) AND json_type(parameters)='object'),
    UNIQUE(id, dimension)
);


CREATE TABLE IF NOT EXISTS structured_objects (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('memory_semantics','concept','episode','journal','schema','association')),
    current_revision INTEGER NOT NULL CHECK(current_revision>0),
    commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
    UNIQUE(namespace_id,subject_id,id),
    FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
    FOREIGN KEY(id,current_revision) REFERENCES structured_revisions(object_id,revision)
        DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX IF NOT EXISTS structured_subject ON structured_objects(namespace_id,subject_id,kind,id);

CREATE TABLE IF NOT EXISTS structured_revisions (
    object_id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision>0),
    kind TEXT NOT NULL CHECK(kind IN ('memory_semantics','concept','episode','journal','schema','association')),
    body TEXT NOT NULL CHECK(json_valid(body) AND json_extract(body,'$.kind')=kind),
    times TEXT NOT NULL CHECK(json_valid(times)),
    producer_call_id TEXT,
    recorded_at TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK(deleted IN(0,1)),
    commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
    PRIMARY KEY(object_id,revision),
    UNIQUE(namespace_id,subject_id,object_id,revision),
    FOREIGN KEY(namespace_id,subject_id,object_id)
        REFERENCES structured_objects(namespace_id,subject_id,id),
    FOREIGN KEY(namespace_id,subject_id,producer_call_id)
        REFERENCES model_calls(namespace_id,subject_id,id)
);
CREATE INDEX IF NOT EXISTS structured_history ON structured_revisions(namespace_id,subject_id,commit_seq,object_id,revision);


CREATE TABLE IF NOT EXISTS structured_links (
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    category TEXT NOT NULL CHECK(category IN('body','evidence','body_evidence')),
    source_kind TEXT NOT NULL CHECK(source_kind IN('material','media','observation','memory','memory_semantics','concept','episode','journal','schema','association')),
    source_id TEXT NOT NULL,
    source_revision INTEGER CHECK(source_revision IS NULL OR source_revision>0),
    polarity TEXT CHECK(polarity IS NULL OR polarity IN('support','contradict','context')),
    relation TEXT,
    explanation TEXT,
    CHECK((source_kind IN('material','media','observation') AND source_revision IS NULL)
       OR (source_kind NOT IN('material','media','observation') AND source_revision IS NOT NULL)),
    PRIMARY KEY(object_id,revision,category,ordinal),
    FOREIGN KEY(namespace_id,subject_id,object_id,revision)
        REFERENCES structured_revisions(namespace_id,subject_id,object_id,revision),
    CHECK((category IN('evidence','body_evidence') AND polarity IS NOT NULL AND explanation IS NOT NULL)
       OR (category='body' AND polarity IS NULL))
);
CREATE INDEX IF NOT EXISTS structured_reference_targets ON structured_links(namespace_id,subject_id,source_kind,source_id,source_revision);

CREATE TABLE IF NOT EXISTS concept_aliases (
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    alias_key TEXT NOT NULL,
    concept_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY(namespace_id,subject_id,alias_key),
    FOREIGN KEY(namespace_id,subject_id,concept_id,revision)
        REFERENCES structured_revisions(namespace_id,subject_id,object_id,revision)
);


CREATE TABLE IF NOT EXISTS structured_formation_groups (
    job_id TEXT NOT NULL REFERENCES structured_formation_runs(job_id),
    key TEXT NOT NULL,
    model_call_id TEXT NOT NULL REFERENCES model_calls(id),
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    depends_on TEXT NOT NULL CHECK(json_valid(depends_on) AND json_type(depends_on)='array'),
    state TEXT NOT NULL CHECK(state IN('pending','applied','rejected','conflict','skipped')),
    result TEXT CHECK(result IS NULL OR json_valid(result)),
    error TEXT CHECK(error IS NULL OR json_valid(error)),
    commit_seq INTEGER CHECK(commit_seq IS NULL OR commit_seq>0),
    PRIMARY KEY(job_id,key),
    UNIQUE(job_id,ordinal),
    CHECK((state='pending' AND commit_seq IS NULL) OR (state<>'pending' AND commit_seq IS NOT NULL))
);


CREATE TABLE IF NOT EXISTS task_records (
 job_id TEXT PRIMARY KEY NOT NULL,namespace_id TEXT,subject_id TEXT,kind TEXT NOT NULL,
 source_read TEXT NOT NULL CHECK(json_valid(source_read) AND json_type(source_read)='array'),
 trace_id TEXT NOT NULL,
 accepted_commit_seq INTEGER NOT NULL CHECK(accepted_commit_seq > 0),
 dependencies TEXT NOT NULL CHECK(json_valid(dependencies) AND json_type(dependencies)='array'),
 state TEXT NOT NULL CHECK(state IN('queued','waiting','running','succeeded','failed','cancelled')),
 wait TEXT CHECK(wait IS NULL OR json_valid(wait)),checkpoint TEXT CHECK(checkpoint IS NULL OR json_valid(checkpoint)),result TEXT CHECK(result IS NULL OR json_valid(result)),
 error TEXT CHECK(error IS NULL OR json_valid(error)),created_at TEXT NOT NULL,updated_at TEXT NOT NULL,
 UNIQUE(job_id,namespace_id,subject_id),
 UNIQUE(job_id,namespace_id,subject_id,trace_id),
 CHECK(subject_id IS NULL OR namespace_id IS NOT NULL),CHECK((state='waiting')=(wait IS NOT NULL)),
 FOREIGN KEY(namespace_id) REFERENCES namespaces(id),FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id)
);
CREATE INDEX IF NOT EXISTS tasks_waiting ON task_records(state,kind,job_id);
CREATE INDEX IF NOT EXISTS tasks_by_trace ON task_records(trace_id,job_id);
CREATE INDEX IF NOT EXISTS calls_by_trace ON model_calls(trace_id,job_id,step_key,attempt);
CREATE TABLE IF NOT EXISTS model_call_redactions (
 call_id TEXT NOT NULL REFERENCES model_calls(id),document TEXT NOT NULL CHECK(document IN('request','response','error')),
 pointer TEXT NOT NULL,replacement TEXT NOT NULL CHECK(json_valid(replacement)),reason TEXT NOT NULL,
 commit_seq INTEGER NOT NULL CHECK(commit_seq>0),PRIMARY KEY(call_id,document,pointer)
);
CREATE TABLE IF NOT EXISTS formation_runs (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),
 observation_id TEXT NOT NULL REFERENCES observations(id),material_id TEXT NOT NULL REFERENCES materials(id),
 source TEXT NOT NULL CHECK(json_valid(source) AND json_type(source)='object'),
 subject_revision INTEGER NOT NULL CHECK(subject_revision>0),material_hash TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS structured_formation_runs (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),
 input TEXT NOT NULL CHECK(json_valid(input) AND json_type(input)='object')
);
CREATE TABLE IF NOT EXISTS embedding_runs (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),
 space_id TEXT NOT NULL REFERENCES embedding_spaces(id),
 content_hashes TEXT NOT NULL CHECK(json_valid(content_hashes) AND json_type(content_hashes)='array'),
 UNIQUE(job_id,space_id)
);
CREATE TABLE IF NOT EXISTS embedding_content_claims (
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,space_id TEXT NOT NULL,source_hash TEXT NOT NULL,job_id TEXT NOT NULL,
 PRIMARY KEY(namespace_id,subject_id,space_id,source_hash),
 FOREIGN KEY(job_id,namespace_id,subject_id) REFERENCES task_records(job_id,namespace_id,subject_id),
 FOREIGN KEY(job_id,space_id) REFERENCES embedding_runs(job_id,space_id)
);
CREATE INDEX IF NOT EXISTS claimed_embedding_job ON embedding_content_claims(job_id,source_hash);
CREATE TABLE IF NOT EXISTS embedding_subscriptions (
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,space_id TEXT NOT NULL REFERENCES embedding_spaces(id),
 source_hash TEXT NOT NULL,target TEXT NOT NULL CHECK(json_valid(target)),source_key TEXT NOT NULL,identity_key TEXT NOT NULL,
 parts TEXT NOT NULL CHECK(json_valid(parts) AND json_type(parts)='array'),
 read_requirements TEXT NOT NULL CHECK(json_valid(read_requirements) AND json_type(read_requirements)='array'),
 accepted_commit_seq INTEGER NOT NULL,
 PRIMARY KEY(namespace_id,subject_id,space_id,source_hash,source_key,accepted_commit_seq),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
 FOREIGN KEY(accepted_commit_seq) REFERENCES operation_receipts(commit_seq) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX IF NOT EXISTS subscription_identity ON embedding_subscriptions(namespace_id,subject_id,identity_key);
CREATE TABLE IF NOT EXISTS embedding_contents (
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,space_id TEXT NOT NULL,
 source_hash TEXT NOT NULL,
 dimension INTEGER NOT NULL CHECK(dimension>0),vector BLOB NOT NULL CHECK(length(vector)=dimension*4),
 model_call_id TEXT NOT NULL,commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 PRIMARY KEY(namespace_id,subject_id,space_id,source_hash),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
 FOREIGN KEY(space_id,dimension) REFERENCES embedding_spaces(id,dimension),
 FOREIGN KEY(namespace_id,subject_id,model_call_id) REFERENCES model_calls(namespace_id,subject_id,id)
);
CREATE TABLE IF NOT EXISTS embedding_records (
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,space_id TEXT NOT NULL,source_hash TEXT NOT NULL,
 target TEXT NOT NULL CHECK(json_valid(target)),source_key TEXT NOT NULL,identity_key TEXT NOT NULL,
 commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 PRIMARY KEY(namespace_id,subject_id,space_id,source_key,source_hash),
 FOREIGN KEY(namespace_id,subject_id,space_id,source_hash) REFERENCES embedding_contents(namespace_id,subject_id,space_id,source_hash)
);
CREATE INDEX IF NOT EXISTS record_identity_history ON embedding_records(namespace_id,subject_id,identity_key,space_id,commit_seq);
CREATE INDEX IF NOT EXISTS record_content ON embedding_records(namespace_id,subject_id,space_id,source_hash,commit_seq);
INSERT OR IGNORE INTO authority_state VALUES(1,0);

CREATE TABLE IF NOT EXISTS media_preparations (
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,
 source TEXT NOT NULL CHECK(json_valid(source) AND json_type(source)='object'),
 plan_digest TEXT NOT NULL CHECK(length(plan_digest)=64 AND plan_digest NOT GLOB '*[^0-9a-f]*'),
 graph TEXT NOT NULL CHECK(json_valid(graph) AND json_type(graph)='object'),
 recorded_at TEXT NOT NULL,commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 PRIMARY KEY(namespace_id,subject_id,source,plan_digest),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id)
);

CREATE TRIGGER IF NOT EXISTS embedding_record_reference BEFORE INSERT ON embedding_records WHEN json_extract(NEW.target,'$.kind')='item' AND json_extract(NEW.target,'$.target.kind')='cognition' BEGIN
 SELECT RAISE(ABORT,'embedding reference is outside scope') WHERE
 (json_extract(NEW.target,'$.target.reference.kind')='memory' AND NOT EXISTS(SELECT 1 FROM memory_revisions WHERE namespace_id=NEW.namespace_id AND subject_id=NEW.subject_id AND memory_id=json_extract(NEW.target,'$.target.reference.id') AND revision=json_extract(NEW.target,'$.target.reference.revision'))) OR
 (json_extract(NEW.target,'$.target.reference.kind')<>'memory' AND NOT EXISTS(SELECT 1 FROM structured_revisions WHERE namespace_id=NEW.namespace_id AND subject_id=NEW.subject_id AND kind=json_extract(NEW.target,'$.target.reference.kind') AND object_id=json_extract(NEW.target,'$.target.reference.id') AND revision=json_extract(NEW.target,'$.target.reference.revision')));
END;

CREATE TABLE IF NOT EXISTS source_regions (
 id TEXT PRIMARY KEY NOT NULL,namespace_id TEXT NOT NULL,material_id TEXT NOT NULL,parent_id TEXT,
 coordinate_hash TEXT NOT NULL,body TEXT NOT NULL CHECK(json_valid(body)),commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 FOREIGN KEY(namespace_id,material_id) REFERENCES materials(namespace_id,id),
 FOREIGN KEY(parent_id) REFERENCES source_regions(id),
 UNIQUE(namespace_id,coordinate_hash),
 CHECK(json_extract(body,'$.id') IS id AND json_extract(body,'$.namespace_id') IS namespace_id
   AND json_extract(body,'$.material_id') IS material_id AND json_extract(body,'$.parent') IS parent_id
   AND json_extract(body,'$.coordinate_hash') IS coordinate_hash AND json_extract(body,'$.commit_seq') IS commit_seq)
);
CREATE TABLE IF NOT EXISTS media_representations (
 id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,
 derivation_key TEXT NOT NULL,body TEXT NOT NULL CHECK(json_valid(body)),commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 model_call_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.producer.model_call_id')) STORED REFERENCES model_calls(id),
 PRIMARY KEY(id,revision),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
 UNIQUE(namespace_id,subject_id,model_call_id),
 CHECK(json_extract(body,'$.id') IS id AND json_extract(body,'$.revision') IS revision
   AND json_extract(body,'$.namespace_id') IS namespace_id AND json_extract(body,'$.subject_id') IS subject_id
   AND json_extract(body,'$.commit_seq') IS commit_seq)
);
CREATE INDEX IF NOT EXISTS media_representations_by_derivation ON media_representations(namespace_id,subject_id,derivation_key);
CREATE INDEX IF NOT EXISTS media_representations_by_scope ON media_representations(namespace_id,subject_id,id,revision DESC);
CREATE TABLE IF NOT EXISTS derived_regions (
 id TEXT PRIMARY KEY NOT NULL,namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,
 representation_id TEXT NOT NULL,representation_revision INTEGER NOT NULL,parent_id TEXT,
 selector TEXT NOT NULL CHECK(json_valid(selector)),body TEXT NOT NULL CHECK(json_valid(body)),commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 FOREIGN KEY(representation_id,representation_revision) REFERENCES media_representations(id,revision),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),FOREIGN KEY(parent_id) REFERENCES derived_regions(id),
 CHECK(json_extract(body,'$.id') IS id AND json_extract(body,'$.namespace_id') IS namespace_id
   AND json_extract(body,'$.subject_id') IS subject_id AND json_extract(body,'$.representation.id') IS representation_id
   AND json_extract(body,'$.representation.revision') IS representation_revision AND json_extract(body,'$.parent') IS parent_id
   AND json_extract(body,'$.commit_seq') IS commit_seq)
);
CREATE UNIQUE INDEX IF NOT EXISTS derived_regions_by_selector ON derived_regions(namespace_id,subject_id,representation_id,representation_revision,coalesce(parent_id,''),selector);
CREATE TABLE IF NOT EXISTS media_derivations (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),request TEXT NOT NULL CHECK(json_valid(request)),
 source_event_kind TEXT NOT NULL CHECK(source_event_kind IN('contact','import')),
 description_target TEXT CHECK(description_target IS NULL OR json_valid(description_target)),
 result_target TEXT NOT NULL CHECK(json_valid(result_target)),prepared TEXT CHECK(prepared IS NULL OR json_valid(prepared)),
 description TEXT CHECK(description IS NULL OR json_valid(description)),result TEXT CHECK(result IS NULL OR json_valid(result))
);
CREATE TABLE IF NOT EXISTS media_exposures (
 observation_id TEXT NOT NULL REFERENCES observations(id),namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,
 locator TEXT NOT NULL CHECK(json_valid(locator)),body TEXT NOT NULL CHECK(json_valid(body)),commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 PRIMARY KEY(observation_id,locator),FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
 CHECK(json_extract(body,'$.observation_id') IS observation_id AND json_extract(body,'$.namespace_id') IS namespace_id
   AND json_extract(body,'$.subject_id') IS subject_id AND json_extract(body,'$.commit_seq') IS commit_seq)
);
