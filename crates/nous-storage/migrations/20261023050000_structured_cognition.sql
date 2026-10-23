-- Scoped identities point to immutable, typed cognition revisions.
CREATE TABLE structured_objects (
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
CREATE INDEX structured_subject ON structured_objects(namespace_id,subject_id,kind,id);

CREATE TABLE structured_revisions (
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
CREATE INDEX structured_history ON structured_revisions(namespace_id,subject_id,commit_seq,object_id,revision);

-- Exact references retain their identity after source retirement or purge.
-- Polymorphic targets are validated under the same SQLite write transaction.
CREATE TABLE structured_links (
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
    source TEXT NOT NULL CHECK(json_valid(source)),
    PRIMARY KEY(object_id,revision,category,ordinal),
    FOREIGN KEY(namespace_id,subject_id,object_id,revision)
        REFERENCES structured_revisions(namespace_id,subject_id,object_id,revision),
    CHECK((category IN('evidence','body_evidence') AND polarity IS NOT NULL AND explanation IS NOT NULL)
       OR (category='body' AND polarity IS NULL))
);
CREATE INDEX structured_reference_targets ON structured_links(namespace_id,subject_id,source_kind,source_id,source_revision);

-- Only the current, live Concept owns its normalized lexical aliases.
CREATE TABLE concept_aliases (
    namespace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    alias_key TEXT NOT NULL,
    concept_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY(namespace_id,subject_id,alias_key),
    FOREIGN KEY(namespace_id,subject_id,concept_id,revision)
        REFERENCES structured_revisions(namespace_id,subject_id,object_id,revision)
);
