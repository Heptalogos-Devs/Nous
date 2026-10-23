CREATE TABLE embedding_spaces (
    id TEXT PRIMARY KEY NOT NULL,
    resource_id TEXT NOT NULL,
    provider_model TEXT NOT NULL,
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    producer_signature TEXT NOT NULL,
    modality TEXT NOT NULL CHECK (modality='text'),
    UNIQUE(id, producer_signature),
    UNIQUE(id, dimension)
);
CREATE TABLE embedding_runs (
    job_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    producer_signature TEXT NOT NULL,
    space_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('queued','waiting_for_configuration','waiting_for_budget','running','succeeded','failed','uncertain','cancelled')),
    model_call_id TEXT,
    error TEXT CHECK (error IS NULL OR json_valid(error)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(job_id, namespace_id, subject_id) REFERENCES model_jobs(job_id, namespace_id, subject_id),
    FOREIGN KEY(space_id, producer_signature) REFERENCES embedding_spaces(id, producer_signature),
    FOREIGN KEY(job_id, model_call_id) REFERENCES model_calls(job_id, id) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX embedding_runs_by_state ON embedding_runs(state, job_id);
CREATE INDEX embedding_claim_scope ON embedding_runs(namespace_id, subject_id, space_id, state, job_id);
CREATE TABLE embedding_inputs (
    job_id TEXT NOT NULL REFERENCES embedding_runs(job_id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('memory','tag','query')),
    source_key TEXT NOT NULL,
    text TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    memory_id TEXT,
    memory_revision INTEGER,
    PRIMARY KEY(job_id, ordinal),
    UNIQUE(job_id, kind, source_key),
    CHECK ((kind='memory' AND memory_id IS NOT NULL AND memory_revision IS NOT NULL AND memory_revision > 0)
        OR (kind IN ('tag','query') AND memory_id IS NULL AND memory_revision IS NULL))
);
CREATE INDEX embedding_input_memory ON embedding_inputs(memory_id, job_id);
CREATE TABLE embedding_records (
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('memory','tag','query')),
    source_key TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    memory_id TEXT,
    memory_revision INTEGER,
    dimension INTEGER NOT NULL CHECK (dimension > 0),
    vector BLOB NOT NULL CHECK (length(vector)=dimension*4),
    model_call_id TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    PRIMARY KEY(namespace_id, subject_id, space_id, kind, source_key),
    CHECK ((kind='memory' AND memory_id IS NOT NULL AND memory_revision IS NOT NULL AND memory_revision > 0)
        OR (kind IN ('tag','query') AND memory_id IS NULL AND memory_revision IS NULL)),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id),
    FOREIGN KEY(space_id, dimension) REFERENCES embedding_spaces(id, dimension),
    FOREIGN KEY(namespace_id, subject_id, model_call_id) REFERENCES model_calls(namespace_id, subject_id, id),
    FOREIGN KEY(memory_id, memory_revision) REFERENCES memory_revisions(memory_id, revision)
);
CREATE INDEX embeddings_by_memory ON embedding_records(namespace_id, memory_id, space_id, memory_revision);
