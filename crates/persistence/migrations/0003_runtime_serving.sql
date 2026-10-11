-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

CREATE FUNCTION authority_recording_time() RETURNS timestamptz LANGUAGE sql VOLATILE AS $$
    SELECT COALESCE(NULLIF(current_setting('nous.authority_time',true),'')::timestamptz,clock_timestamp())
$$;
CREATE TABLE mutation_receipts (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    operation_id uuid NOT NULL,
    operation_kind text NOT NULL,
    request_digest text NOT NULL CHECK (request_digest ~ '^[0-9a-f]{64}$'),
    state text NOT NULL CHECK (state IN ('in_progress','committed','failed_terminal')),
    result_kind text NULL,
    result_ref_kind text NULL,
    result_ref text NULL,
    result_revision uuid NULL,
    result_epoch bigint NULL,
    created_at timestamptz NOT NULL,
    committed_at timestamptz NULL,
    terminal_problem_code text NULL,
    PRIMARY KEY(subject_id, operation_id)
);
CREATE TABLE cognition_dependency_invalidations (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    dependent_kind text NOT NULL,
    dependent_ref text NOT NULL,
    invalidated_by_kind text NOT NULL,
    invalidated_by_ref text NOT NULL,
    reason text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY(dependent_kind, dependent_ref, invalidated_by_kind, invalidated_by_ref)
);

CREATE TABLE work_contexts (
    work_context_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    state text NOT NULL CHECK (state IN ('open','paused','ended')),
    purpose text NOT NULL CHECK (octet_length(purpose) BETWEEN 1 AND 8192),
    context_text text NOT NULL DEFAULT '' CHECK (octet_length(context_text) <= 65536),
    unresolved_questions text[] NOT NULL DEFAULT '{}',
    constraints jsonb NOT NULL DEFAULT '{}',
    resume_conditions text[] NOT NULL DEFAULT '{}',
    budget_summary jsonb NOT NULL DEFAULT '{}',
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    ended_at timestamptz NULL,
    UNIQUE(subject_id, work_context_id),
    CHECK (cardinality(unresolved_questions) <= 64),
    CHECK (cardinality(resume_conditions) <= 64),
    CHECK (octet_length(constraints::text) <= 65536),
    CHECK (octet_length(budget_summary::text) <= 16384),
    CHECK ((state = 'ended') = (ended_at IS NOT NULL))
);
CREATE TABLE work_context_refs (
    work_context_id uuid NOT NULL REFERENCES work_contexts(work_context_id) ON DELETE CASCADE,
    ordinal integer NOT NULL CHECK (ordinal >= 0),
    ref_kind text NOT NULL CHECK (ref_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    ref_value text NOT NULL,
    PRIMARY KEY(work_context_id, ordinal),
    UNIQUE(work_context_id, ref_kind, ref_value)
);

CREATE TABLE work_context_anchors (
    work_context_id uuid NOT NULL REFERENCES work_contexts(work_context_id) ON DELETE CASCADE,
    ordinal integer NOT NULL CHECK (ordinal >= 0),
    ref_kind text NOT NULL CHECK (ref_kind IN ('entity','tag')),
    ref_value text NOT NULL,
    PRIMARY KEY(work_context_id, ordinal),
    UNIQUE(work_context_id, ref_kind, ref_value)
);

CREATE TABLE cognitive_sessions (
    session_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    opened_at timestamptz NOT NULL,
    last_activity_at timestamptz NOT NULL,
    last_meaningful_use_at timestamptz NULL,
    closed_at timestamptz NULL,
    runtime_revision bigint NOT NULL DEFAULT 0,
    active_work_context_id uuid NULL,
    FOREIGN KEY(subject_id, active_work_context_id)
        REFERENCES work_contexts(subject_id, work_context_id),
    metadata jsonb NOT NULL DEFAULT '{}'
);

CREATE TABLE resident_refs (
    session_id uuid NOT NULL REFERENCES cognitive_sessions(session_id) ON DELETE CASCADE,
    ref_kind text NOT NULL CHECK (ref_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    ref_value text NOT NULL,
    entered_at timestamptz NOT NULL,
    entry_reason text NOT NULL,
    last_meaningful_use_at timestamptz NULL,
    hold_until timestamptz NULL,
    state text NOT NULL CHECK (state IN ('resident','provisional','evicted')),
    metadata jsonb NOT NULL DEFAULT '{}',
    PRIMARY KEY(session_id, ref_kind, ref_value)
);

CREATE TABLE maintenance_needs (
    need_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    scope_kind text NOT NULL CHECK (scope_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    scope_ref text NOT NULL CHECK (octet_length(scope_ref) BETWEEN 1 AND 512),
    trigger_authority_seq bigint NOT NULL CHECK (trigger_authority_seq >= 0),
    trigger_revision bigint NOT NULL DEFAULT 1 CHECK (trigger_revision > 0),
    due_at timestamptz NOT NULL,
    priority integer NOT NULL CHECK (priority BETWEEN 0 AND 100),
    state text NOT NULL CHECK (state IN ('pending','leased','blocked','satisfied','obsolete')),
    lease_token uuid NULL,
    lease_until timestamptz NULL,
    last_ack_token uuid NULL,
    last_ack_digest text NULL CHECK (last_ack_digest IS NULL OR last_ack_digest ~ '^[0-9a-f]{64}$'),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    retry_count integer NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    retry_not_before timestamptz NULL,
    terminal_at timestamptz NULL,
    blocked_config_digest text NULL,
    model_execution_digest text NULL CHECK (model_execution_digest IS NULL OR model_execution_digest ~ '^[0-9a-f]{64}$'),
    last_problem_code text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CHECK ((state = 'leased') = (lease_token IS NOT NULL AND lease_until IS NOT NULL))
);
CREATE UNIQUE INDEX maintenance_active_scope ON maintenance_needs(subject_id,kind,scope_kind,scope_ref)
    WHERE state IN ('pending','leased','blocked');
CREATE INDEX maintenance_due ON maintenance_needs(subject_id,due_at,priority DESC)
    WHERE state IN ('pending','leased');

CREATE TABLE model_workflow_operations (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    owner text NOT NULL CHECK (owner ~ '^[a-z][a-z0-9_]{0,63}$'),
    operation_key text NOT NULL CHECK (length(operation_key) BETWEEN 1 AND 256),
    semantic_digest text NOT NULL CHECK (length(semantic_digest) BETWEEN 1 AND 128),
    snapshot jsonb NOT NULL,
    proposal jsonb NULL,
    outcome jsonb NULL,
    execution_telemetry jsonb NULL,
    maintenance_trigger_revision bigint NULL CHECK (maintenance_trigger_revision >= 0),
    maintenance_need_id uuid NULL REFERENCES maintenance_needs(need_id) ON DELETE CASCADE,
    lease_token uuid NULL,
    lease_until timestamptz NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(subject_id,owner,operation_key)
);

CREATE INDEX maintenance_workflows ON model_workflow_operations(maintenance_need_id) WHERE maintenance_need_id IS NOT NULL;

CREATE TABLE experience_items (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    recorded_seq bigint NOT NULL CHECK (recorded_seq > 0),
    occurrence_id uuid NOT NULL UNIQUE REFERENCES observation_occurrences(occurrence_id) ON DELETE CASCADE,
    session_id uuid NOT NULL REFERENCES cognitive_sessions(session_id),
    observed_at timestamptz NOT NULL,
    source_class text NOT NULL,
    conversation_ref text NULL,
    actor_entity_ref text NULL,
    active_work_context_id uuid NULL,
    active_work_context_revision bigint NULL,
    PRIMARY KEY(subject_id,recorded_seq),
    FOREIGN KEY(subject_id,active_work_context_id) REFERENCES work_contexts(subject_id,work_context_id),
    CHECK ((active_work_context_id IS NULL) = (active_work_context_revision IS NULL))
);

CREATE INDEX experience_work_context ON experience_items(subject_id,active_work_context_id) WHERE active_work_context_id IS NOT NULL;
CREATE TABLE episode_drafts (
    draft_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    track_key text NOT NULL CHECK (track_key ~ '^[a-z][a-z0-9_]{0,63}$'),
    state text NOT NULL CHECK (state IN ('open','ready')),
    first_recorded_seq bigint NOT NULL,
    last_recorded_seq bigint NOT NULL,
    observed_start timestamptz NOT NULL,
    observed_end timestamptz NOT NULL,
    boundary_reason text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CHECK (first_recorded_seq <= last_recorded_seq),
    CHECK (observed_start <= observed_end)
);
CREATE UNIQUE INDEX episode_open_draft ON episode_drafts(subject_id,track_key) WHERE state='open';
CREATE TABLE episode_draft_members (
    draft_id uuid NOT NULL REFERENCES episode_drafts(draft_id) ON DELETE CASCADE,
    recorded_seq bigint NOT NULL,
    occurrence_id uuid NOT NULL REFERENCES observation_occurrences(occurrence_id) ON DELETE CASCADE,
    PRIMARY KEY(draft_id,recorded_seq),
    UNIQUE(draft_id,occurrence_id)
);
CREATE TABLE segmentation_cursors (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    track_key text NOT NULL CHECK (track_key ~ '^[a-z][a-z0-9_]{0,63}$'),
    last_recorded_seq bigint NOT NULL DEFAULT 0 CHECK (last_recorded_seq >= 0),
    open_draft_id uuid NULL REFERENCES episode_drafts(draft_id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id,track_key)
);
CREATE TABLE query_feedback_records (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    query_id uuid NOT NULL,
    session_id uuid NULL REFERENCES cognitive_sessions(session_id) ON DELETE SET NULL,
    work_context_id uuid NULL REFERENCES work_contexts(work_context_id) ON DELETE SET NULL,
    prepared_query_digest text NOT NULL CHECK (prepared_query_digest ~ '^[0-9a-f]{64}$'),
    query_activation_digest text NOT NULL CHECK (query_activation_digest ~ '^[0-9a-f]{64}$'),
    signals jsonb NOT NULL CHECK (octet_length(signals::text)<=65536),
    returned_revision_refs jsonb NOT NULL CHECK (jsonb_typeof(returned_revision_refs)='array' AND jsonb_array_length(returned_revision_refs)<=256),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at>created_at),
    PRIMARY KEY(subject_id,query_id)
);
CREATE INDEX query_feedback_expiry ON query_feedback_records(subject_id,expires_at);

CREATE TABLE cognitive_use_events (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    consumer_ref text NOT NULL CHECK (octet_length(consumer_ref) BETWEEN 1 AND 512 AND consumer_ref !~ '[[:space:]]' AND consumer_ref LIKE '%:%:%'),
    event_id uuid NOT NULL,
    query_id uuid NULL,
    ref_kind text NOT NULL CHECK (ref_kind IN ('memory_revision','cognitive_schema_revision','episode_revision','journal_revision')),
    ref_value text NOT NULL,
    use_kind text NOT NULL CHECK (use_kind IN ('presented','referenced','acted_on','result_supported','result_refuted','corrected','pinned')),
    session_id uuid NULL REFERENCES cognitive_sessions(session_id) ON DELETE SET NULL,
    occurred_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL,
    context jsonb NOT NULL DEFAULT '{}',
    request_digest text NOT NULL CHECK (request_digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY(subject_id, consumer_ref, event_id)
);
CREATE INDEX cognitive_use_query_feedback ON cognitive_use_events(subject_id,ref_kind,ref_value,recorded_at DESC) WHERE query_id IS NOT NULL;
CREATE TABLE purged_use_receipts (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    consumer_ref text NOT NULL,
    event_id uuid NOT NULL,
    request_digest text NOT NULL CHECK (request_digest ~ '^[0-9a-f]{64}$'),
    purged_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id, consumer_ref, event_id)
);

CREATE TABLE serving_generations (
    view_descriptor jsonb NOT NULL DEFAULT '{"kind":"current"}' CHECK (view_descriptor->>'kind' IN ('current','historical')),
    generation_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    family text NOT NULL CHECK (family IN ('exact','lexical','dense','concept','topology')),
    space_signature text NULL,
    authority_watermark bigint NOT NULL,
    implementation_id text NOT NULL,
    implementation_revision text NOT NULL,
    config_digest text NOT NULL,
    artifact_location text NOT NULL,
    artifact_hash text NOT NULL,
    state text NOT NULL CHECK (state IN ('building','ready','retired','failed')),
    built_at timestamptz NOT NULL,
    published_at timestamptz NULL,
    metadata jsonb NOT NULL DEFAULT '{}'
);
CREATE TABLE serving_current (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    family text NOT NULL,
    space_signature text NOT NULL DEFAULT '',
    generation_id uuid NOT NULL REFERENCES serving_generations(generation_id) ON DELETE CASCADE,
    PRIMARY KEY(subject_id, family, space_signature)
);
CREATE TABLE projection_watermarks (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    family text NOT NULL,
    space_signature text NOT NULL DEFAULT '',
    desired_authority_seq bigint NOT NULL,
    PRIMARY KEY(subject_id, family, space_signature)
);

CREATE TABLE observation_request_bindings (
    request_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id),
    request_digest text NOT NULL,
    occurrence_id uuid NOT NULL REFERENCES observation_occurrences(occurrence_id),
    accepted jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE embedding_materials (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id),
    content_digest text NOT NULL,
    space_hash text NOT NULL,
    producer_hash text NOT NULL,
    vector real[] NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (subject_id, content_digest, space_hash, producer_hash)
);

CREATE TABLE lexical_bindings (
    lexical_ref text PRIMARY KEY,
    object_kind text NOT NULL,
    canonical_ref text NOT NULL,
    tombstoned_at timestamptz NULL,
    created_at timestamptz NOT NULL DEFAULT authority_recording_time(),
    UNIQUE (object_kind, canonical_ref)
);
CREATE TABLE lexical_visibility (
    lexical_ref text NOT NULL REFERENCES lexical_bindings(lexical_ref) ON UPDATE CASCADE,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id),
    display_name text NOT NULL,
    aliases text[] NOT NULL,
    PRIMARY KEY(subject_id, lexical_ref)
);
CREATE INDEX lexical_aliases ON lexical_visibility USING gin(aliases);

-- Minimal canonical state chronology; immutable revision content is never copied here.
CREATE TABLE authority_object_states (
    event_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    object_kind text NOT NULL,
    object_ref text NOT NULL,
    state jsonb NOT NULL,
    recorded_at timestamptz NOT NULL
);
CREATE INDEX authority_object_state_as_of ON authority_object_states(subject_id,object_kind,object_ref,recorded_at DESC,event_id DESC);
CREATE FUNCTION capture_authority_object_state() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    payload jsonb := to_jsonb(NEW);
    captured timestamptz := authority_recording_time();
BEGIN
    IF TG_OP='INSERT' OR to_jsonb(OLD) IS DISTINCT FROM payload THEN
        INSERT INTO authority_object_states(subject_id,object_kind,object_ref,state,recorded_at)
        VALUES(NEW.subject_id,TG_ARGV[0],payload->>TG_ARGV[1],payload,captured);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER memory_state_history AFTER INSERT OR UPDATE ON memory_objects FOR EACH ROW EXECUTE FUNCTION capture_authority_object_state('memory','memory_id');
CREATE TRIGGER schema_state_history AFTER INSERT OR UPDATE ON cognitive_schemas FOR EACH ROW EXECUTE FUNCTION capture_authority_object_state('cognitive_schema','schema_id');
CREATE TRIGGER episode_state_history AFTER INSERT OR UPDATE ON episode_objects FOR EACH ROW EXECUTE FUNCTION capture_authority_object_state('episode','episode_id');
CREATE TRIGGER journal_state_history AFTER INSERT OR UPDATE ON journal_objects FOR EACH ROW EXECUTE FUNCTION capture_authority_object_state('journal','journal_id');
CREATE TRIGGER tag_state_history AFTER INSERT OR UPDATE ON tags FOR EACH ROW EXECUTE FUNCTION capture_authority_object_state('tag','tag_id');
CREATE TRIGGER lexical_state_history AFTER INSERT OR UPDATE ON lexical_visibility FOR EACH ROW EXECUTE FUNCTION capture_authority_object_state('lexical_visibility','lexical_ref');
