-- Stable memory identity and immutable revision snapshots.
CREATE UNIQUE INDEX observations_scoped_source
ON observations(namespace_id, subject_id, id, material_id);

CREATE TABLE formation_runs (
    job_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    material_id TEXT NOT NULL,
    subject_revision INTEGER NOT NULL CHECK (subject_revision > 0),
    material_hash TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('queued','waiting_for_configuration','waiting_for_budget','running','succeeded','failed','uncertain','cancelled')),
    model_call_id TEXT,
    error TEXT CHECK (error IS NULL OR json_valid(error)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(job_id, namespace_id, subject_id),
    FOREIGN KEY(namespace_id, subject_id, observation_id, material_id)
        REFERENCES observations(namespace_id, subject_id, id, material_id),
    FOREIGN KEY(job_id) REFERENCES Jobs(id) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY(job_id, model_call_id) REFERENCES model_calls(job_id, id) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX formations_by_state ON formation_runs(state, job_id);

CREATE TABLE model_calls (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    resource_id TEXT NOT NULL,
    request TEXT NOT NULL CHECK (json_valid(request)),
    response TEXT CHECK (response IS NULL OR json_valid(response)),
    state TEXT NOT NULL CHECK (state IN ('started','succeeded','failed','uncertain')),
    error TEXT CHECK (error IS NULL OR json_valid(error)),
    cost_microunits INTEGER CHECK (cost_microunits IS NULL OR cost_microunits >= 0),
    created_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(job_id, attempt),
    UNIQUE(job_id, id),
    UNIQUE(namespace_id, subject_id, id),
    FOREIGN KEY(job_id, namespace_id, subject_id)
        REFERENCES formation_runs(job_id, namespace_id, subject_id)
);
CREATE INDEX model_calls_daily_resource ON model_calls(resource_id, created_at);

CREATE TABLE memories (
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
CREATE INDEX memories_by_subject ON memories(namespace_id, subject_id, commit_seq, id);

CREATE TABLE memory_revisions (
    memory_id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    epistemic_status TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    material_id TEXT NOT NULL,
    model_call_id TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    times TEXT NOT NULL CHECK (json_valid(times)),
    recorded_at TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0,1)),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    PRIMARY KEY(memory_id, revision),
    FOREIGN KEY(namespace_id, subject_id, memory_id)
        REFERENCES memories(namespace_id, subject_id, id),
    FOREIGN KEY(namespace_id, subject_id, observation_id, material_id)
        REFERENCES observations(namespace_id, subject_id, id, material_id),
    FOREIGN KEY(namespace_id, subject_id, model_call_id)
        REFERENCES model_calls(namespace_id, subject_id, id)
);
CREATE INDEX memory_changes ON memory_revisions(commit_seq, memory_id, revision);
CREATE INDEX subject_memory_changes ON memory_revisions(namespace_id, subject_id, commit_seq, memory_id);

CREATE TABLE formation_memories (
    job_id TEXT NOT NULL REFERENCES formation_runs(job_id),
    memory_id TEXT NOT NULL REFERENCES memories(id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY(job_id, memory_id),
    UNIQUE(job_id, ordinal)
);
