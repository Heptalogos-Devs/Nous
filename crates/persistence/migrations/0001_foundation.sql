-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

CREATE TABLE subjects (
    subject_id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL,
    authority_seq bigint NOT NULL DEFAULT 0,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','suspended','purging')),
    metadata jsonb NOT NULL DEFAULT '{}'
);

CREATE TABLE artifacts (
    artifact_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    content_hash text NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    byte_length bigint NOT NULL CHECK (byte_length >= 0),
    media_type text NOT NULL,
    storage_key text NOT NULL,
    created_at timestamptz NOT NULL,
    metadata jsonb NOT NULL DEFAULT '{}',
    UNIQUE(subject_id, content_hash)
);


CREATE TABLE observation_occurrences (
    occurrence_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    artifact_id uuid NULL REFERENCES artifacts(artifact_id) ON DELETE RESTRICT,
    source_class text NOT NULL,
    external_object_ref text NULL,
    occurred_time_kind text NOT NULL DEFAULT 'unknown'
        CHECK (occurred_time_kind IN ('unknown','instant','interval')),
    occurred_time_start timestamptz NULL,
    occurred_time_end timestamptz NULL,
    observed_at timestamptz NOT NULL,
    conversation_ref text NULL,
    actor_entity_ref text NULL,
    context jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL,
    CHECK (occurred_time_kind <> 'interval'
        OR occurred_time_start IS NULL
        OR occurred_time_end IS NULL
        OR occurred_time_start < occurred_time_end)
);
CREATE INDEX observation_occurrences_subject_observed_idx
    ON observation_occurrences(subject_id, observed_at DESC);

CREATE TABLE source_regions (
    source_region_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    artifact_id uuid NOT NULL REFERENCES artifacts(artifact_id) ON DELETE CASCADE,
    coordinate_kind text NOT NULL,
    coordinate jsonb NOT NULL,
    coordinate_hash text NOT NULL,
    parent_source_region_id uuid NULL REFERENCES source_regions(source_region_id),
    created_at timestamptz NOT NULL,
    UNIQUE(subject_id, artifact_id, coordinate_kind, coordinate_hash)
);

CREATE TABLE producer_signatures (
    producer_signature_id uuid PRIMARY KEY,
    signature_hash text UNIQUE NOT NULL,
    provider_class text NOT NULL,
    operation text NOT NULL,
    implementation text NOT NULL,
    model_identity text NULL,
    model_revision text NULL,
    model_role text NULL,
    model_profile text NULL,
    execution_profile text NULL,
    inference_controls_digest text NULL,
    role_policy_digest text NULL,
    prompt_id text NULL,
    prompt_digest text NULL,
    output_schema_digest text NULL CHECK (output_schema_digest ~ '^[0-9a-f]{64}$'),
    preprocessing_identity text NOT NULL,
    preprocessing_revision text NOT NULL,
    config_digest text NOT NULL,
    created_at timestamptz NOT NULL,
    metadata jsonb NOT NULL DEFAULT '{}'
);

CREATE TABLE embedding_spaces (
    embedding_space_id uuid PRIMARY KEY,
    space_hash text UNIQUE NOT NULL,
    model_identity text NOT NULL,
    weights_revision text NOT NULL,
    task text NOT NULL,
    input_representation text NOT NULL,
    preprocessing_identity text NOT NULL,
    preprocessing_revision text NOT NULL,
    dimension integer NOT NULL CHECK (dimension > 0),
    normalization text NOT NULL,
    output_semantics text NOT NULL,
    created_at timestamptz NOT NULL
);

CREATE TABLE derived_representations (
    derived_representation_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    input_digest text NOT NULL,
    strategy text NOT NULL,
    derivation_key text UNIQUE NOT NULL,
    representation_kind text NOT NULL,
    producer_signature_id uuid NOT NULL REFERENCES producer_signatures(producer_signature_id),
    revision integer NOT NULL CHECK (revision > 0),
    payload_text text NULL,
    payload_json jsonb NULL CHECK (jsonb_typeof(payload_json) = 'object'),
    payload_artifact_id uuid NULL REFERENCES artifacts(artifact_id),
    quality jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL,
    supersedes uuid NULL REFERENCES derived_representations(derived_representation_id),
    CHECK (payload_text IS NOT NULL OR payload_json IS NOT NULL OR payload_artifact_id IS NOT NULL)
);

CREATE TABLE derived_regions (
    derived_region_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    derived_representation_id uuid NOT NULL REFERENCES derived_representations(derived_representation_id) ON DELETE CASCADE,
    coordinate_kind text NOT NULL,
    coordinate jsonb NOT NULL,
    coordinate_hash text NOT NULL,
    parent_derived_region_id uuid NULL REFERENCES derived_regions(derived_region_id),
    created_at timestamptz NOT NULL,
    UNIQUE(derived_representation_id, coordinate_kind, coordinate_hash)
);

CREATE TABLE derived_representation_inputs (
    derived_representation_id uuid NOT NULL REFERENCES derived_representations(derived_representation_id) ON DELETE CASCADE,
    ordinal integer NOT NULL CHECK (ordinal >= 0),
    role text NOT NULL CHECK (length(role) > 0),
    source_region_id uuid NULL REFERENCES source_regions(source_region_id) DEFERRABLE INITIALLY DEFERRED,
    input_representation_id uuid NULL REFERENCES derived_representations(derived_representation_id) DEFERRABLE INITIALLY DEFERRED,
    derived_region_id uuid NULL REFERENCES derived_regions(derived_region_id) DEFERRABLE INITIALLY DEFERRED,
    PRIMARY KEY (derived_representation_id, ordinal),
    UNIQUE NULLS NOT DISTINCT (derived_representation_id, source_region_id, input_representation_id, derived_region_id),
    CHECK (num_nonnulls(source_region_id, input_representation_id, derived_region_id) = 1),
    CHECK (input_representation_id IS DISTINCT FROM derived_representation_id)
);

CREATE FUNCTION representation_source_regions(owner_subject uuid, representation uuid)
RETURNS TABLE (source_region_id uuid)
LANGUAGE SQL STABLE AS $$
    WITH RECURSIVE ancestry(id) AS (
        SELECT derived_representation_id FROM derived_representations
        WHERE subject_id = owner_subject AND derived_representation_id = representation
        UNION
        SELECT COALESCE(i.input_representation_id, r.derived_representation_id)
        FROM ancestry a JOIN derived_representation_inputs i ON i.derived_representation_id = a.id
        LEFT JOIN derived_regions r ON r.derived_region_id = i.derived_region_id
        WHERE i.input_representation_id IS NOT NULL OR r.derived_representation_id IS NOT NULL
    )
    SELECT DISTINCT i.source_region_id FROM ancestry a
    JOIN derived_representation_inputs i ON i.derived_representation_id = a.id
    WHERE i.source_region_id IS NOT NULL
$$;

CREATE TABLE coverage_needs (
    coverage_need_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    source_region_id uuid NOT NULL REFERENCES source_regions(source_region_id) ON DELETE CASCADE,
    representation_kind text NOT NULL,
    capability_operation text NOT NULL,
    requirement text NOT NULL CHECK (requirement IN ('required','preferred','opportunistic')),
    state text NOT NULL CHECK (state IN ('missing','scheduled','ready','unavailable','failed')),
    current_representation_id uuid NULL REFERENCES derived_representations(derived_representation_id),
    updated_at timestamptz NOT NULL,
    UNIQUE(subject_id, source_region_id, representation_kind, capability_operation)
);

CREATE TABLE entity_mentions (
    mention_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    occurrence_id uuid NULL REFERENCES observation_occurrences(occurrence_id) ON DELETE CASCADE,
    source_region_id uuid NULL REFERENCES source_regions(source_region_id) ON DELETE CASCADE,
    derived_region_id uuid NULL REFERENCES derived_regions(derived_region_id) ON DELETE CASCADE,
    surface text NOT NULL,
    semantic_role text NULL,
    created_at timestamptz NOT NULL,
    CHECK (num_nonnulls(occurrence_id, source_region_id, derived_region_id) >= 1)
);

CREATE TABLE entity_binding_revisions (
    binding_revision_id uuid PRIMARY KEY,
    mention_id uuid NOT NULL REFERENCES entity_mentions(mention_id) ON DELETE CASCADE,
    revision_no integer NOT NULL CHECK (revision_no > 0),
    entity_ref text NULL,
    binding_state text NOT NULL CHECK (binding_state IN ('bound','unbound','disputed')),
    host_resolution_ref text NULL,
    reason text NULL,
    created_at timestamptz NOT NULL,
    UNIQUE(mention_id, revision_no)
);

CREATE TABLE resources (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    resource_ref text NOT NULL,
    adapter_kind text NOT NULL,
    provider_profile text NOT NULL,
    provider_locator text NOT NULL,
    display_label text NULL,
    authority_class text NOT NULL,
    coverage jsonb NOT NULL DEFAULT '{}',
    query_dimensions jsonb NOT NULL DEFAULT '{}',
    modalities jsonb NOT NULL DEFAULT '[]',
    freshness_policy jsonb NOT NULL DEFAULT '{}',
    access_cost_class text NOT NULL,
    readiness text NOT NULL CHECK (readiness IN ('ready','degraded','unavailable')),
    updated_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id, resource_ref)
);

CREATE INDEX entity_binding_current_lookup_idx
    ON entity_binding_revisions(mention_id, revision_no DESC);

CREATE TABLE cognitive_seed_versions (
    seed_version_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    artifact_id uuid NOT NULL REFERENCES artifacts(artifact_id) ON DELETE RESTRICT,
    format text NOT NULL CHECK (format = 'application/vnd.nous-wave.cognitive-seed+toml;version=1'),
    provenance jsonb NOT NULL DEFAULT '{}',
    content_hash text NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    created_at timestamptz NOT NULL,
    UNIQUE(subject_id, content_hash, format)
);

CREATE TABLE subject_seed_adoptions (
    adoption_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    seed_version_id uuid NOT NULL REFERENCES cognitive_seed_versions(seed_version_id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK (kind IN ('initial','import')),
    operation_id uuid NOT NULL,
    request_digest text NOT NULL CHECK (request_digest ~ '^[0-9a-f]{64}$'),
    adopted_at timestamptz NOT NULL,
    UNIQUE(subject_id, operation_id),
    UNIQUE(subject_id, seed_version_id, kind)
);
CREATE TABLE configuration_state (
    singleton boolean PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    revision bigint NOT NULL CHECK (revision > 0)
);
INSERT INTO configuration_state(singleton, revision) VALUES(TRUE, 1);

CREATE TABLE system_configuration_overrides (
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    updated_at timestamptz NOT NULL
);

CREATE TABLE subject_configuration_overrides (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    key text NOT NULL,
    value jsonb NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    updated_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id, key)
);

CREATE TABLE configuration_mutation_receipts (
    operation_id uuid PRIMARY KEY,
    request_digest text NOT NULL CHECK (request_digest ~ '^[0-9a-f]{64}$'),
    subject_id uuid NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    key text NOT NULL,
    action text NOT NULL CHECK (action IN ('set','clear')),
    revision bigint NOT NULL CHECK (revision > 0),
    apply_mode text NOT NULL CHECK (apply_mode IN ('live','serving_rebuild','new_subjects_only','restart_process')),
    pending_restart boolean NOT NULL,
    active_digest text NOT NULL CHECK (active_digest ~ '^[0-9a-f]{64}$'),
    desired_digest text NOT NULL CHECK (desired_digest ~ '^[0-9a-f]{64}$'),
    created_at timestamptz NOT NULL
);

CREATE TABLE subject_capabilities (
    subject_id uuid PRIMARY KEY REFERENCES subjects(subject_id) ON DELETE CASCADE,
    memory boolean NOT NULL
);
