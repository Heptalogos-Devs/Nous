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
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0), kind TEXT NOT NULL CHECK (kind IN ('contact','import')),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id),
    FOREIGN KEY(namespace_id, material_id) REFERENCES materials(namespace_id, id),
    CHECK ((observed_at_domain IS NULL) = (observed_at_coordinate IS NULL))
);
CREATE INDEX IF NOT EXISTS observations_by_subject ON observations(namespace_id, subject_id, commit_seq, id);
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
    resource_id TEXT NOT NULL,
    provider_model TEXT NOT NULL,
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    producer_signature TEXT NOT NULL,
    modality TEXT NOT NULL CHECK (modality='text'),
    UNIQUE(id, producer_signature),
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
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    source_revision INTEGER CHECK(source_revision IS NULL OR source_revision>0),
    polarity TEXT CHECK(polarity IS NULL OR polarity IN('support','contradict','context')),
    relation TEXT,
    explanation TEXT,
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
CREATE TABLE IF NOT EXISTS formation_runs (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),
 observation_id TEXT NOT NULL REFERENCES observations(id),material_id TEXT NOT NULL REFERENCES materials(id),
 subject_revision INTEGER NOT NULL CHECK(subject_revision>0),material_hash TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS structured_formation_runs (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),
 input TEXT NOT NULL CHECK(json_valid(input) AND json_type(input)='object')
);
CREATE TABLE IF NOT EXISTS embedding_runs (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),
 producer_signature TEXT NOT NULL,space_id TEXT NOT NULL,
 FOREIGN KEY(space_id,producer_signature) REFERENCES embedding_spaces(id,producer_signature)
);
CREATE TABLE IF NOT EXISTS embedding_inputs (
 job_id TEXT NOT NULL REFERENCES embedding_runs(job_id),ordinal INTEGER NOT NULL CHECK(ordinal>=0),
 target_kind TEXT NOT NULL CHECK(target_kind IN('cognition','tag','query')),
 cognition_kind TEXT,cognition_id TEXT,cognition_revision INTEGER,text TEXT NOT NULL,
 PRIMARY KEY(job_id,ordinal),
 CHECK((target_kind='cognition' AND cognition_kind IS NOT NULL AND cognition_id IS NOT NULL AND cognition_revision IS NOT NULL AND cognition_revision>0)
 OR(target_kind IN('tag','query') AND cognition_kind IS NULL AND cognition_id IS NULL AND cognition_revision IS NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS input_cognition ON embedding_inputs(job_id,cognition_kind,cognition_id,cognition_revision) WHERE target_kind='cognition';
CREATE UNIQUE INDEX IF NOT EXISTS input_text ON embedding_inputs(job_id,target_kind,text) WHERE target_kind IN('tag','query');
CREATE TABLE IF NOT EXISTS embedding_records (
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,space_id TEXT NOT NULL,
 target_kind TEXT NOT NULL CHECK(target_kind IN('cognition','tag','query')),
 cognition_kind TEXT,cognition_id TEXT,cognition_revision INTEGER,source_hash TEXT NOT NULL,
 dimension INTEGER NOT NULL CHECK(dimension>0),vector BLOB NOT NULL CHECK(length(vector)=dimension*4),
 model_call_id TEXT NOT NULL,commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 CHECK((target_kind='cognition' AND cognition_kind IS NOT NULL AND cognition_id IS NOT NULL AND cognition_revision IS NOT NULL AND cognition_revision>0)
 OR(target_kind IN('tag','query') AND cognition_kind IS NULL AND cognition_id IS NULL AND cognition_revision IS NULL)),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
 FOREIGN KEY(space_id,dimension) REFERENCES embedding_spaces(id,dimension),
 FOREIGN KEY(namespace_id,subject_id,model_call_id) REFERENCES model_calls(namespace_id,subject_id,id)
);
CREATE UNIQUE INDEX IF NOT EXISTS record_cognition ON embedding_records(namespace_id,subject_id,space_id,cognition_kind,cognition_id,cognition_revision) WHERE target_kind='cognition';
CREATE UNIQUE INDEX IF NOT EXISTS record_text ON embedding_records(namespace_id,subject_id,space_id,target_kind,source_hash) WHERE target_kind IN('tag','query');
CREATE INDEX IF NOT EXISTS record_cognition_history ON embedding_records(namespace_id,subject_id,cognition_id,space_id,cognition_revision);
INSERT OR IGNORE INTO authority_state VALUES(1,0);

CREATE TRIGGER IF NOT EXISTS embedding_record_reference BEFORE INSERT ON embedding_records WHEN NEW.target_kind='cognition' BEGIN
 SELECT RAISE(ABORT,'embedding reference is outside scope') WHERE
 (NEW.cognition_kind='memory' AND NOT EXISTS(SELECT 1 FROM memory_revisions WHERE namespace_id=NEW.namespace_id AND subject_id=NEW.subject_id AND memory_id=NEW.cognition_id AND revision=NEW.cognition_revision)) OR
 (NEW.cognition_kind<>'memory' AND NOT EXISTS(SELECT 1 FROM structured_revisions WHERE namespace_id=NEW.namespace_id AND subject_id=NEW.subject_id AND kind=NEW.cognition_kind AND object_id=NEW.cognition_id AND revision=NEW.cognition_revision));
END;
