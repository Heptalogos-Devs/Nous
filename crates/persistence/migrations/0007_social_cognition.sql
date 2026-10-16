-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

ALTER TABLE subject_capabilities ADD COLUMN social_cognition boolean NOT NULL DEFAULT false;
CREATE TABLE social_objects (
 object_id uuid PRIMARY KEY,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 family text NOT NULL CHECK(family IN ('social_relation','language_convention')),
 current_revision_id uuid NOT NULL,
 identity_key text NOT NULL,
 UNIQUE(subject_id,family,identity_key),
 object_epoch bigint NOT NULL DEFAULT 1 CHECK(object_epoch>0),
 acceptance_state text NOT NULL DEFAULT 'accepted' CHECK(acceptance_state IN ('accepted','withdrawn')),
 integrity_state text NOT NULL DEFAULT 'valid' CHECK(integrity_state IN ('valid','revalidation_required')),
 suppression_state text NOT NULL DEFAULT 'normal' CHECK(suppression_state IN ('normal','suppressed')),
 created_at timestamptz NOT NULL,
 UNIQUE(object_id,subject_id)
);
CREATE TABLE social_revisions (
 revision_id uuid PRIMARY KEY,
 object_id uuid NOT NULL REFERENCES social_objects(object_id) ON DELETE CASCADE,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 revision_no integer NOT NULL CHECK(revision_no>0),
 parent_revision_id uuid REFERENCES social_revisions(revision_id),
 content jsonb NOT NULL CHECK(jsonb_typeof(content)='object'),
 reason text NOT NULL,
 formed_at timestamptz NOT NULL,
 recorded_at timestamptz NOT NULL,
 UNIQUE(object_id,revision_no)
);
ALTER TABLE social_objects ADD CONSTRAINT social_current_revision_fk FOREIGN KEY(current_revision_id)
 REFERENCES social_revisions(revision_id) DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE social_evidence (
 revision_id uuid NOT NULL REFERENCES social_revisions(revision_id) ON DELETE CASCADE,
 role text NOT NULL CHECK(role IN ('support','counterexample','boundary_case')),
 basis_key text NOT NULL,
 basis jsonb NOT NULL,
 recorded_at timestamptz NOT NULL,
 PRIMARY KEY(revision_id,role,basis_key)
);
CREATE INDEX social_subject_current ON social_objects(subject_id,family,object_id);
CREATE INDEX social_revision_history ON social_revisions(object_id,revision_no);
CREATE TABLE social_dependencies (
 revision_id uuid NOT NULL REFERENCES social_revisions(revision_id) ON DELETE CASCADE,
 ref_kind text NOT NULL,
 ref_value text NOT NULL,
 recorded_at timestamptz NOT NULL,
 PRIMARY KEY(revision_id,ref_kind,ref_value)
);
CREATE INDEX social_dependencies_source ON social_dependencies(ref_kind,ref_value);

ALTER TABLE cognitive_use_events DROP CONSTRAINT cognitive_use_events_ref_kind_check;
ALTER TABLE cognitive_use_events ADD CONSTRAINT cognitive_use_events_ref_kind_check
 CHECK(ref_kind IN ('memory_revision','cognitive_schema_revision','episode_revision','journal_revision','self_facet_revision','narrative_thread_revision','social_relation_revision','language_convention_revision'));
CREATE TRIGGER social_state_history AFTER INSERT OR UPDATE ON social_objects FOR EACH ROW
 EXECUTE FUNCTION capture_authority_object_state('social','object_id');

-- Rebuildable, derived from exact basis and evidence through owning lineage readers.
CREATE TABLE social_source_roots (
 revision_id uuid NOT NULL REFERENCES social_revisions(revision_id) ON DELETE CASCADE,
 root_key text NOT NULL,
 certainty text NOT NULL CHECK(certainty IN ('known','occurrence_only')),
 PRIMARY KEY(revision_id,root_key,certainty)
);
CREATE INDEX social_source_root_lookup ON social_source_roots(root_key,revision_id);
