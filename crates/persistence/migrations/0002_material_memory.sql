-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

CREATE TABLE memory_objects (
    memory_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    cognitive_role text NOT NULL CHECK (cognitive_role IN ('experiential','declarative','procedural_experience')),
    current_revision_id uuid NOT NULL,
    object_epoch bigint NOT NULL DEFAULT 1 CHECK (object_epoch > 0),
    acceptance_state text NOT NULL CHECK (acceptance_state IN ('accepted','withdrawn')),
    integrity_state text NOT NULL CHECK (integrity_state IN ('valid','revalidation_required')),
    suppression_state text NOT NULL CHECK (suppression_state IN ('normal','suppressed')),
    purge_state text NOT NULL CHECK (purge_state IN ('normal','purging')),
    accessibility_mode text NOT NULL DEFAULT 'auto' CHECK (accessibility_mode IN ('auto','normal','deep','explicit')),
    created_at timestamptz NOT NULL
);

CREATE TABLE memory_revisions (
    memory_revision_id uuid PRIMARY KEY,
    memory_id uuid NOT NULL REFERENCES memory_objects(memory_id) ON DELETE CASCADE,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    revision_no integer NOT NULL CHECK (revision_no > 0),
    parent_revision_id uuid NULL REFERENCES memory_revisions(memory_revision_id),
    revision_intent text NULL CHECK (revision_intent IN ('correct','rephrase','reinterpret')),
    formation_mode text NOT NULL CHECK (formation_mode IN ('grounded','synthesized')),
    grounding_occurrence_id uuid NULL REFERENCES observation_occurrences(occurrence_id) ON DELETE RESTRICT,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    semantic_role text NOT NULL,
    title text NULL,
    representation_text text NOT NULL CHECK (length(trim(representation_text)) > 0 AND octet_length(representation_text) <= 1048576),
    epistemic_class text NOT NULL,
    valid_time_kind text NOT NULL DEFAULT 'unknown' CHECK (valid_time_kind IN ('unknown','instant','interval')),
    valid_time_start timestamptz NULL,
    valid_time_end timestamptz NULL,
    formed_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL,
    UNIQUE(memory_id, revision_no),
    CHECK (valid_time_kind <> 'interval'
        OR valid_time_start IS NULL
        OR valid_time_end IS NULL
        OR valid_time_start < valid_time_end),
    CHECK ((formation_mode = 'grounded') = (grounding_occurrence_id IS NOT NULL))
);
ALTER TABLE memory_objects
    ADD CONSTRAINT memory_objects_current_revision_fk
    FOREIGN KEY (current_revision_id) REFERENCES memory_revisions(memory_revision_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE memory_revision_evidence (
    memory_revision_id uuid NOT NULL REFERENCES memory_revisions(memory_revision_id) ON DELETE CASCADE,
    occurrence_id uuid NOT NULL REFERENCES observation_occurrences(occurrence_id) ON DELETE RESTRICT,
    evidence_no integer NOT NULL CHECK (evidence_no >= 0),
    source_region_id uuid NULL REFERENCES source_regions(source_region_id) ON DELETE RESTRICT,
    derived_representation_id uuid NULL REFERENCES derived_representations(derived_representation_id) ON DELETE RESTRICT,
    derived_region_id uuid NULL REFERENCES derived_regions(derived_region_id) ON DELETE RESTRICT,
    basis_role text NOT NULL CHECK (basis_role IN ('direct','interpretation','contextual')),
    epistemic_relation text NOT NULL DEFAULT '' CHECK (epistemic_relation IN ('','supports','contradicts','corroborates','weakens','corrects','counterexample','inferred_from')),
    PRIMARY KEY(memory_revision_id, evidence_no),
    CHECK (num_nonnulls(source_region_id, derived_representation_id, derived_region_id) <= 1)
);

CREATE TABLE memory_revision_dependencies (
    memory_revision_id uuid NOT NULL REFERENCES memory_revisions(memory_revision_id) ON DELETE CASCADE,
    target_ref_kind text NOT NULL CHECK (target_ref_kind IN ('memory_revision','cognitive_schema_revision','episode_revision','journal_revision')),
    target_ref text NOT NULL,
    basis_role text NOT NULL CHECK (basis_role IN ('direct','interpretation','contextual')),
    epistemic_relation text NOT NULL DEFAULT '' CHECK (epistemic_relation IN ('','supports','contradicts','corroborates','weakens','corrects','counterexample','inferred_from')),
    PRIMARY KEY(memory_revision_id, target_ref_kind, target_ref, basis_role, epistemic_relation)
);

CREATE TABLE memory_revision_aboutness (
    memory_revision_id uuid NOT NULL REFERENCES memory_revisions(memory_revision_id) ON DELETE CASCADE,
    entity_ref text NOT NULL,
    PRIMARY KEY(memory_revision_id, entity_ref)
);

CREATE TABLE memory_revision_relations (
    from_revision_id uuid NOT NULL REFERENCES memory_revisions(memory_revision_id) ON DELETE CASCADE,
    to_revision_id uuid NOT NULL REFERENCES memory_revisions(memory_revision_id) ON DELETE CASCADE,
    relation text NOT NULL CHECK (relation IN ('derived_from','contradicts','temporal_successor','replaces_basis','elaborates')),
    created_at timestamptz NOT NULL,
    PRIMARY KEY(from_revision_id, to_revision_id, relation)
);

CREATE TABLE tags (
    tag_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    current_revision_id uuid NOT NULL,
    created_at timestamptz NOT NULL,
    status text NOT NULL CHECK (status IN ('active','withdrawn','merged')),
    canonical_tag_id uuid NULL,
    UNIQUE(tag_id, subject_id),
    FOREIGN KEY(canonical_tag_id, subject_id) REFERENCES tags(tag_id, subject_id),
    CHECK ((status='merged') = (canonical_tag_id IS NOT NULL)),
    CHECK (canonical_tag_id IS NULL OR canonical_tag_id <> tag_id)
);
CREATE FUNCTION canonical_tag(subject uuid, requested uuid) RETURNS uuid
LANGUAGE sql STABLE AS $$
    SELECT COALESCE(t.canonical_tag_id,t.tag_id) FROM tags t
    LEFT JOIN tags survivor ON survivor.tag_id=t.canonical_tag_id AND survivor.subject_id=t.subject_id
    WHERE t.subject_id=subject AND t.tag_id=requested
      AND (t.status='active' OR (t.status='merged' AND survivor.status='active'))
$$;

CREATE TABLE tag_revisions (
    tag_revision_id uuid PRIMARY KEY,
    tag_id uuid NOT NULL REFERENCES tags(tag_id) ON DELETE CASCADE,
    revision_no integer NOT NULL CHECK (revision_no > 0),
    label text NOT NULL,
    description text NULL,
    kind_hint text NULL,
    origin text NOT NULL,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    created_at timestamptz NOT NULL,
    UNIQUE(tag_id, revision_no)
);
ALTER TABLE tags ADD CONSTRAINT tags_current_revision_fk
    FOREIGN KEY (current_revision_id) REFERENCES tag_revisions(tag_revision_id)
    DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE tag_lineage (
    lineage_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    operation_id uuid NOT NULL,
    parent_tag_id uuid NOT NULL,
    child_tag_id uuid NOT NULL,
    parent_revision_id uuid NOT NULL REFERENCES tag_revisions(tag_revision_id),
    child_revision_id uuid NOT NULL REFERENCES tag_revisions(tag_revision_id),
    child_index integer NOT NULL,
    relation text NOT NULL CHECK(relation IN ('merged_into','split_into')),
    basis jsonb NOT NULL CHECK(jsonb_typeof(basis)='array' AND jsonb_array_length(basis) BETWEEN 1 AND 16),
    created_at timestamptz NOT NULL,
    FOREIGN KEY(parent_tag_id,subject_id) REFERENCES tags(tag_id,subject_id),
    FOREIGN KEY(child_tag_id,subject_id) REFERENCES tags(tag_id,subject_id),
    UNIQUE(subject_id,operation_id,parent_tag_id,child_tag_id,relation),
    CHECK(parent_tag_id<>child_tag_id)
);
CREATE TABLE memory_revision_tags (
    memory_revision_id uuid NOT NULL REFERENCES memory_revisions(memory_revision_id) ON DELETE CASCADE,
    tag_id uuid NOT NULL REFERENCES tags(tag_id) ON DELETE RESTRICT,
    PRIMARY KEY(memory_revision_id, tag_id)
);

CREATE TABLE cognitive_schemas (
    schema_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    current_revision_id uuid NOT NULL,
    object_epoch bigint NOT NULL DEFAULT 1 CHECK (object_epoch > 0),
    acceptance_state text NOT NULL CHECK (acceptance_state IN ('accepted','withdrawn')),
    integrity_state text NOT NULL CHECK (integrity_state IN ('valid','revalidation_required')),
    suppression_state text NOT NULL CHECK (suppression_state IN ('normal','suppressed')),
    purge_state text NOT NULL CHECK (purge_state IN ('normal','purging')),
    created_at timestamptz NOT NULL
);
CREATE TABLE cognitive_schema_revisions (
    schema_revision_id uuid PRIMARY KEY,
    schema_id uuid NOT NULL REFERENCES cognitive_schemas(schema_id) ON DELETE CASCADE,
    revision_no integer NOT NULL CHECK (revision_no > 0),
    parent_revision_id uuid NULL REFERENCES cognitive_schema_revisions(schema_revision_id),
    revision_intent text NULL CHECK (revision_intent IN ('correct','rephrase','reinterpret')),
    formation_kind text NOT NULL DEFAULT 'explicit_import'
        CHECK (formation_kind IN ('explicit_import','synthesized')),
    title text NULL,
    structural_claim text NOT NULL CHECK (octet_length(structural_claim) <= 16384),
    applicability_description text NOT NULL CHECK (octet_length(applicability_description) <= 16384),
    aboutness text[] NOT NULL DEFAULT '{}',
    tags uuid[] NOT NULL DEFAULT '{}',
    boundary_definition text NOT NULL CHECK (octet_length(boundary_definition) <= 16384),
    valid_time_kind text NOT NULL DEFAULT 'unknown' CHECK (valid_time_kind IN ('unknown','instant','interval')),
    valid_time_start timestamptz NULL,
    valid_time_end timestamptz NULL,
    formed_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    UNIQUE(schema_id, revision_no),
    CHECK (valid_time_kind <> 'interval'
        OR valid_time_start IS NULL
        OR valid_time_end IS NULL
        OR valid_time_start < valid_time_end)
);
ALTER TABLE cognitive_schemas
    ADD CONSTRAINT cognitive_schemas_current_revision_fk
    FOREIGN KEY (current_revision_id) REFERENCES cognitive_schema_revisions(schema_revision_id)
    DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE cognitive_schema_evidence_links (
    link_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    schema_revision_id uuid NOT NULL REFERENCES cognitive_schema_revisions(schema_revision_id) ON DELETE CASCADE,
    role text NOT NULL CHECK (role IN ('support','counterexample','boundary_case')),
    basis_kind text NOT NULL CHECK (basis_kind IN ('evidence','memory_revision','cognitive_schema_revision','episode_revision','journal_revision')),
    basis_ref text NOT NULL,
    basis_role text NOT NULL CHECK (basis_role IN ('direct','interpretation','contextual')),
    epistemic_relation text NOT NULL DEFAULT '' CHECK (epistemic_relation IN ('','supports','contradicts','corroborates','weakens','corrects','counterexample','inferred_from')),
    occurrence_id uuid NULL REFERENCES observation_occurrences(occurrence_id) ON DELETE RESTRICT,
    source_region_id uuid NULL REFERENCES source_regions(source_region_id) ON DELETE RESTRICT,
    derived_representation_id uuid NULL REFERENCES derived_representations(derived_representation_id) ON DELETE RESTRICT,
    derived_region_id uuid NULL REFERENCES derived_regions(derived_region_id) ON DELETE RESTRICT,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    created_at timestamptz NOT NULL,
    revoked_at timestamptz NULL
);
CREATE TABLE cognitive_schema_lineage (
    from_revision_id uuid NOT NULL REFERENCES cognitive_schema_revisions(schema_revision_id) ON DELETE CASCADE,
    to_revision_id uuid NOT NULL REFERENCES cognitive_schema_revisions(schema_revision_id) ON DELETE CASCADE,
    relation text NOT NULL CHECK (relation IN ('schema_split_from','schema_merged_from')),
    PRIMARY KEY(from_revision_id, to_revision_id, relation)
);

CREATE TABLE association_evidence (
    association_evidence_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    from_ref_kind text NOT NULL,
    from_ref text NOT NULL,
    to_ref_kind text NOT NULL,
    to_ref text NOT NULL,
    relation_kind text NOT NULL CHECK (length(relation_kind) BETWEEN 1 AND 128 AND relation_kind ~ '^[a-z0-9_.:-]+$'),
    polarity text NOT NULL CHECK (polarity IN ('positive','negative')),
    basis_class text NOT NULL CHECK (basis_class IN ('host_explicit','source_evidence','cognitive_derivation','meaningful_use','derived_structure')),
    valid_time_kind text NOT NULL DEFAULT 'unknown' CHECK (valid_time_kind IN ('unknown','instant','interval')),
    valid_time_start timestamptz NULL,
    valid_time_end timestamptz NULL,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    created_at timestamptz NOT NULL,
    revoked_at timestamptz NULL
);
CREATE TABLE association_evidence_basis (
    association_evidence_id uuid NOT NULL REFERENCES association_evidence(association_evidence_id) ON DELETE CASCADE,
    basis_kind text NOT NULL CHECK (basis_kind IN ('evidence','memory_revision','cognitive_schema_revision','episode_revision','journal_revision','use_event')),
    basis_ref text NOT NULL,
    basis_role text NOT NULL CHECK (basis_role IN ('direct','interpretation','contextual')),
    epistemic_relation text NOT NULL DEFAULT '' CHECK (epistemic_relation IN ('','supports','contradicts','corroborates','weakens','corrects','counterexample','inferred_from')),
    occurrence_id uuid NULL REFERENCES observation_occurrences(occurrence_id) ON DELETE RESTRICT,
    source_region_id uuid NULL REFERENCES source_regions(source_region_id) ON DELETE RESTRICT,
    derived_representation_id uuid NULL REFERENCES derived_representations(derived_representation_id) ON DELETE RESTRICT,
    derived_region_id uuid NULL REFERENCES derived_regions(derived_region_id) ON DELETE RESTRICT,
    PRIMARY KEY(association_evidence_id, basis_kind, basis_ref, basis_role, epistemic_relation),
    CHECK (num_nonnulls(source_region_id, derived_representation_id, derived_region_id) <= 1)
);

CREATE TABLE episode_objects (
    episode_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    track_key text NOT NULL CHECK (octet_length(track_key) BETWEEN 1 AND 128),
    current_revision_id uuid NOT NULL,
    object_epoch bigint NOT NULL DEFAULT 1 CHECK (object_epoch > 0),
    acceptance_state text NOT NULL CHECK (acceptance_state IN ('accepted','withdrawn')),
    integrity_state text NOT NULL CHECK (integrity_state IN ('valid','revalidation_required')),
    suppression_state text NOT NULL CHECK (suppression_state IN ('normal','suppressed')),
    purge_state text NOT NULL CHECK (purge_state IN ('normal','purging')),
    created_at timestamptz NOT NULL,
    UNIQUE(subject_id, episode_id)
);
CREATE TABLE episode_revisions (
    episode_revision_id uuid PRIMARY KEY,
    episode_id uuid NOT NULL REFERENCES episode_objects(episode_id) ON DELETE CASCADE,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    revision_no integer NOT NULL CHECK (revision_no > 0),
    parent_revision_id uuid NULL REFERENCES episode_revisions(episode_revision_id),
    revision_intent text NULL CHECK (revision_intent IN ('resegment','reinterpret')),
    title text NULL CHECK (title IS NULL OR octet_length(title) <= 8192),
    parent_episode_revision_id uuid NULL REFERENCES episode_revisions(episode_revision_id) ON DELETE SET NULL,
    experience_time_kind text NOT NULL DEFAULT 'unknown' CHECK (experience_time_kind IN ('unknown','instant','interval')),
    experience_time_start timestamptz NULL,
    experience_time_end timestamptz NULL,
    boundary_explanation text NOT NULL CHECK (octet_length(boundary_explanation) BETWEEN 1 AND 16384),
    formed_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    UNIQUE(episode_id, revision_no),
    CHECK (experience_time_kind <> 'interval'
        OR experience_time_start IS NULL
        OR experience_time_end IS NULL
        OR experience_time_start < experience_time_end)
);
ALTER TABLE episode_objects
    ADD CONSTRAINT episode_objects_current_revision_fk
    FOREIGN KEY (current_revision_id) REFERENCES episode_revisions(episode_revision_id)
    DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE episode_revision_members (
    episode_revision_id uuid NOT NULL REFERENCES episode_revisions(episode_revision_id) ON DELETE CASCADE,
    ordinal integer NOT NULL CHECK (ordinal >= 0),
    ref_kind text NOT NULL CHECK (ref_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    ref_value text NOT NULL,
    role text NOT NULL CHECK (octet_length(role) BETWEEN 1 AND 128),
    PRIMARY KEY(episode_revision_id, ordinal),
    UNIQUE(episode_revision_id, ref_kind, ref_value)
);
CREATE TABLE episode_revision_basis (
    episode_revision_id uuid NOT NULL REFERENCES episode_revisions(episode_revision_id) ON DELETE CASCADE,
    basis_no integer NOT NULL CHECK (basis_no >= 0),
    basis_kind text NOT NULL CHECK (basis_kind IN ('evidence','memory_revision','cognitive_schema_revision','episode_revision')),
    basis_ref text NOT NULL,
    basis_role text NOT NULL CHECK (basis_role IN ('direct','interpretation','contextual')),
    epistemic_relation text NOT NULL DEFAULT '' CHECK (epistemic_relation IN ('','supports','contradicts','corroborates','weakens','corrects','counterexample','inferred_from')),
    occurrence_id uuid NULL REFERENCES observation_occurrences(occurrence_id) ON DELETE RESTRICT,
    source_region_id uuid NULL REFERENCES source_regions(source_region_id) ON DELETE RESTRICT,
    derived_representation_id uuid NULL REFERENCES derived_representations(derived_representation_id) ON DELETE RESTRICT,
    derived_region_id uuid NULL REFERENCES derived_regions(derived_region_id) ON DELETE RESTRICT,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    PRIMARY KEY(episode_revision_id, basis_no),
    CHECK (num_nonnulls(source_region_id, derived_representation_id, derived_region_id) <= 1)
);
CREATE TABLE episode_revision_relations (
    from_revision_id uuid NOT NULL REFERENCES episode_revisions(episode_revision_id) ON DELETE CASCADE,
    to_revision_id uuid NOT NULL REFERENCES episode_revisions(episode_revision_id) ON DELETE CASCADE,
    relation text NOT NULL CHECK (relation IN ('split_from','merged_from','temporal_successor','derived_from')),
    created_at timestamptz NOT NULL,
    PRIMARY KEY(from_revision_id, to_revision_id, relation)
);

CREATE TABLE journal_objects (
    journal_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    current_revision_id uuid NOT NULL,
    object_epoch bigint NOT NULL DEFAULT 1 CHECK (object_epoch > 0),
    acceptance_state text NOT NULL CHECK (acceptance_state IN ('accepted','withdrawn')),
    integrity_state text NOT NULL CHECK (integrity_state IN ('valid','revalidation_required')),
    suppression_state text NOT NULL CHECK (suppression_state IN ('normal','suppressed')),
    purge_state text NOT NULL CHECK (purge_state IN ('normal','purging')),
    created_at timestamptz NOT NULL,
    UNIQUE(subject_id,journal_id)
);
CREATE TABLE journal_revisions (
    journal_revision_id uuid PRIMARY KEY,
    journal_id uuid NOT NULL REFERENCES journal_objects(journal_id) ON DELETE CASCADE,
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    revision_no integer NOT NULL CHECK (revision_no > 0),
    parent_revision_id uuid NULL REFERENCES journal_revisions(journal_revision_id),
    revision_intent text NULL CHECK (revision_intent IN ('revalidate','reinterpret','reframe')),
    title text NULL CHECK (title IS NULL OR octet_length(title) <= 8192),
    temporal_scope_kind text NOT NULL CHECK (temporal_scope_kind IN ('unknown','instant','interval')),
    temporal_scope_start timestamptz NULL,
    temporal_scope_end timestamptz NULL,
    narrative text NOT NULL CHECK (octet_length(narrative) BETWEEN 1 AND 131072),
    formed_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL,
    producer_signature_id uuid NULL REFERENCES producer_signatures(producer_signature_id),
    UNIQUE(journal_id,revision_no),
    CHECK (temporal_scope_kind<>'interval' OR temporal_scope_start IS NULL OR temporal_scope_end IS NULL OR temporal_scope_start<temporal_scope_end)
);
ALTER TABLE journal_objects ADD CONSTRAINT journal_current_revision_fk
    FOREIGN KEY(current_revision_id) REFERENCES journal_revisions(journal_revision_id) DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE journal_revision_points (
    journal_revision_id uuid NOT NULL REFERENCES journal_revisions(journal_revision_id) ON DELETE CASCADE,
    ordinal integer NOT NULL CHECK (ordinal BETWEEN 0 AND 127),
    role text NOT NULL CHECK (role IN ('summary','outcome','change','decision','open_question','salient_event','reflection')),
    text text NOT NULL CHECK (octet_length(text) BETWEEN 1 AND 8192),
    PRIMARY KEY(journal_revision_id,ordinal)
);
CREATE TABLE journal_point_basis (
    journal_revision_id uuid NOT NULL,
    ordinal integer NOT NULL,
    basis_no integer NOT NULL CHECK (basis_no BETWEEN 0 AND 15),
    basis jsonb NOT NULL CHECK (jsonb_typeof(basis)='object'),
    PRIMARY KEY(journal_revision_id,ordinal,basis_no),
    FOREIGN KEY(journal_revision_id,ordinal) REFERENCES journal_revision_points(journal_revision_id,ordinal) ON DELETE CASCADE
);
CREATE TABLE journal_revision_sources (
    journal_revision_id uuid NOT NULL REFERENCES journal_revisions(journal_revision_id) ON DELETE CASCADE,
    ref_kind text NOT NULL CHECK (ref_kind IN ('occurrence','memory_revision','cognitive_schema_revision','episode_revision','journal_revision')),
    ref_value text NOT NULL,
    source_epoch bigint NULL CHECK (source_epoch IS NULL OR source_epoch>0),
    PRIMARY KEY(journal_revision_id,ref_kind,ref_value)
);
CREATE INDEX journal_dependency_source ON journal_revision_sources(ref_kind,ref_value);
