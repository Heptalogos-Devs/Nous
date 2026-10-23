-- Import admission records provenance without asserting actual Subject contact.
ALTER TABLE observations ADD COLUMN kind TEXT NOT NULL DEFAULT 'contact' CHECK (kind IN ('contact','import'));

-- Reinsert snapshots under the original table name so the deferred current
-- revision references settle within SQLx's atomic migration transaction.
CREATE TABLE memory_revisions_stage AS SELECT * FROM memory_revisions;
DROP TABLE memory_revisions;
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
    model_call_id TEXT,
    prompt_version TEXT,
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
    CHECK ((origin='model' AND model_call_id IS NOT NULL AND prompt_version IS NOT NULL)
        OR (origin IN ('imported','manual') AND model_call_id IS NULL AND prompt_version IS NULL)),
    PRIMARY KEY(memory_id, revision),
    FOREIGN KEY(namespace_id, subject_id, memory_id) REFERENCES memories(namespace_id, subject_id, id),
    FOREIGN KEY(namespace_id, subject_id, observation_id, material_id) REFERENCES observations(namespace_id, subject_id, id, material_id),
    FOREIGN KEY(namespace_id, subject_id, model_call_id) REFERENCES model_calls(namespace_id, subject_id, id)
);
INSERT INTO memory_revisions (
    memory_id,namespace_id,subject_id,revision,title,content,epistemic_status,
    observation_id,material_id,model_call_id,prompt_version,times,recorded_at,deleted,commit_seq,
    lifecycle_strength,lifecycle_anchor_unix_ms,lifecycle_activation,lifecycle_pinned,
    lifecycle_protected_until_unix_ms,lifecycle_inactive_since_unix_ms,lifecycle_last_reinforced_unix_ms,origin
)
SELECT memory_id,namespace_id,subject_id,revision,title,content,epistemic_status,
    observation_id,material_id,model_call_id,prompt_version,times,recorded_at,deleted,commit_seq,
    1.0, COALESCE(
    CASE WHEN json_extract(times, '$.formation.domain') = 'unix_ms'
         THEN CAST(json_extract(times, '$.formation.coordinate') AS INTEGER) END,
    CAST(round((julianday(recorded_at) - 2440587.5) * 86400000.0) AS INTEGER),
    0
), 'active', 0, NULL, NULL, NULL, 'model'
FROM memory_revisions_stage;
DROP TABLE memory_revisions_stage;
CREATE INDEX memory_changes ON memory_revisions(commit_seq, memory_id, revision);
CREATE INDEX subject_memory_changes ON memory_revisions(namespace_id, subject_id, commit_seq, memory_id);
CREATE INDEX maintenance_by_identity ON memories(namespace_id, subject_id, id);

CREATE TABLE memory_audit (
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
CREATE INDEX audit_by_memory ON memory_audit(namespace_id, memory_id, commit_seq, id);

-- Content-free invalidations outlive a permanently erased Memory.
CREATE TABLE memory_purges (
    memory_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id)
);
CREATE INDEX purge_changes ON memory_purges(commit_seq, memory_id);
CREATE INDEX subject_purge_changes ON memory_purges(namespace_id, subject_id, commit_seq, memory_id);

CREATE TABLE material_details (
    material_id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL,
    source_type TEXT,
    tags TEXT NOT NULL CHECK (json_valid(tags)),
    metadata TEXT NOT NULL CHECK (json_valid(metadata)),
    structured TEXT CHECK (structured IS NULL OR json_valid(structured)),
    FOREIGN KEY(namespace_id, material_id) REFERENCES materials(namespace_id, id)
);
