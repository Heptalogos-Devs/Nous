-- One call ledger serves formation, embedding and subsequent model workloads.
CREATE TABLE model_jobs (
    job_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    UNIQUE(job_id, namespace_id, subject_id),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id),
    FOREIGN KEY(job_id) REFERENCES Jobs(id) DEFERRABLE INITIALLY DEFERRED
);
INSERT INTO model_jobs(job_id,namespace_id,subject_id,kind)
SELECT job_id,namespace_id,subject_id,'formation' FROM formation_runs;

-- Existing memory revisions reference these exact call identities. Reinsert
-- under the original table name and settle all references in this transaction.
PRAGMA defer_foreign_keys = ON;
CREATE TABLE model_calls_stage AS SELECT * FROM model_calls;
DROP TABLE model_calls;
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
        REFERENCES model_jobs(job_id, namespace_id, subject_id)
);
INSERT INTO model_calls SELECT * FROM model_calls_stage;
DROP TABLE model_calls_stage;
CREATE INDEX model_calls_daily_resource ON model_calls(resource_id, created_at);
