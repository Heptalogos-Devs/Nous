CREATE TABLE structured_formation_runs (
    job_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN('queued','waiting_for_configuration','waiting_for_budget','running','succeeded','failed','uncertain','cancelled')),
    input TEXT NOT NULL CHECK(json_valid(input) AND json_type(input)='object'),
    output TEXT CHECK(output IS NULL OR json_valid(output)),
    model_call_id TEXT,
    error TEXT CHECK(error IS NULL OR json_valid(error)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(job_id,namespace_id,subject_id) REFERENCES model_jobs(job_id,namespace_id,subject_id),
    FOREIGN KEY(job_id,model_call_id) REFERENCES model_calls(job_id,id) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX structured_formation_state ON structured_formation_runs(state,job_id);

CREATE TABLE structured_formation_groups (
    job_id TEXT NOT NULL REFERENCES structured_formation_runs(job_id),
    key TEXT NOT NULL,
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
