CREATE TABLE authority_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    commit_seq INTEGER NOT NULL CHECK (commit_seq >= 0)
);
INSERT INTO authority_state VALUES (1, 0);

CREATE TABLE namespaces (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0)
);

CREATE TABLE subjects (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL REFERENCES namespaces(id),
    name TEXT NOT NULL,
    capabilities TEXT NOT NULL CHECK (json_valid(capabilities)),
    revision INTEGER NOT NULL CHECK (revision > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    UNIQUE(namespace_id, id)
);
CREATE INDEX subjects_by_namespace ON subjects(namespace_id, commit_seq, id);

CREATE TABLE materials (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL REFERENCES namespaces(id),
    content_hash TEXT NOT NULL,
    media_type TEXT NOT NULL,
    byte_len INTEGER NOT NULL CHECK (byte_len >= 0),
    source TEXT,
    event_time_domain TEXT,
    event_time_coordinate INTEGER,
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    UNIQUE(namespace_id, id),
    CHECK ((event_time_domain IS NULL) = (event_time_coordinate IS NULL))
);
CREATE INDEX materials_by_namespace ON materials(namespace_id, commit_seq, id);

CREATE TABLE observations (
    id TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL REFERENCES namespaces(id),
    subject_id TEXT NOT NULL,
    material_id TEXT NOT NULL,
    observed_at_domain TEXT,
    observed_at_coordinate INTEGER,
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
    FOREIGN KEY(namespace_id, subject_id) REFERENCES subjects(namespace_id, id),
    FOREIGN KEY(namespace_id, material_id) REFERENCES materials(namespace_id, id),
    CHECK ((observed_at_domain IS NULL) = (observed_at_coordinate IS NULL))
);
CREATE INDEX observations_by_subject ON observations(namespace_id, subject_id, commit_seq, id);

CREATE TABLE operation_receipts (
    caller_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    scope TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    commit_seq INTEGER NOT NULL UNIQUE,
    receipt TEXT NOT NULL CHECK (json_valid(receipt)),
    PRIMARY KEY(caller_id, operation, scope, operation_id)
);
