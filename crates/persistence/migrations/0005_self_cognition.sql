-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

ALTER TABLE subject_capabilities ADD COLUMN self_cognition boolean NOT NULL DEFAULT false;
CREATE TABLE self_objects (
 object_id uuid PRIMARY KEY,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 family text NOT NULL CHECK(family IN ('self_facet','narrative_thread')),
 current_revision_id uuid NOT NULL,
 object_epoch bigint NOT NULL DEFAULT 1 CHECK(object_epoch>0),
 acceptance_state text NOT NULL DEFAULT 'accepted' CHECK(acceptance_state IN ('accepted','withdrawn')),
 integrity_state text NOT NULL DEFAULT 'valid' CHECK(integrity_state IN ('valid','revalidation_required')),
 suppression_state text NOT NULL DEFAULT 'normal' CHECK(suppression_state IN ('normal','suppressed')),
 created_at timestamptz NOT NULL,
 UNIQUE(object_id,subject_id)
);
CREATE TABLE self_revisions (
 revision_id uuid PRIMARY KEY,
 object_id uuid NOT NULL REFERENCES self_objects(object_id) ON DELETE CASCADE,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 revision_no integer NOT NULL CHECK(revision_no>0),
 parent_revision_id uuid REFERENCES self_revisions(revision_id),
 content jsonb NOT NULL CHECK(jsonb_typeof(content)='object'),
 reason text NOT NULL,
 formed_at timestamptz NOT NULL,
 recorded_at timestamptz NOT NULL,
 UNIQUE(object_id,revision_no)
);
ALTER TABLE self_objects ADD CONSTRAINT self_current_revision_fk FOREIGN KEY(current_revision_id)
 REFERENCES self_revisions(revision_id) DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE self_evidence (
 revision_id uuid NOT NULL REFERENCES self_revisions(revision_id) ON DELETE CASCADE,
 role text NOT NULL CHECK(role IN ('support','counterexample','boundary_case')),
 basis_key text NOT NULL,
 basis jsonb NOT NULL,
 recorded_at timestamptz NOT NULL,
 PRIMARY KEY(revision_id,role,basis_key)
);
CREATE INDEX self_subject_current ON self_objects(subject_id,family,object_id);
CREATE INDEX self_revision_history ON self_revisions(object_id,revision_no);
CREATE TABLE self_dependencies (
 revision_id uuid NOT NULL REFERENCES self_revisions(revision_id) ON DELETE CASCADE,
 ref_kind text NOT NULL,
 ref_value text NOT NULL,
 recorded_at timestamptz NOT NULL,
 PRIMARY KEY(revision_id,ref_kind,ref_value)
);
CREATE INDEX self_dependencies_source ON self_dependencies(ref_kind,ref_value);

ALTER TABLE cognitive_seed_versions DROP CONSTRAINT cognitive_seed_versions_format_check;
ALTER TABLE cognitive_seed_versions ADD CONSTRAINT cognitive_seed_versions_format_check
 CHECK(format IN ('application/vnd.nous-wave.cognitive-seed+toml;version=1','text/markdown'));

ALTER TABLE cognitive_use_events DROP CONSTRAINT cognitive_use_events_ref_kind_check;
ALTER TABLE cognitive_use_events ADD CONSTRAINT cognitive_use_events_ref_kind_check
 CHECK(ref_kind IN ('memory_revision','cognitive_schema_revision','episode_revision','journal_revision','self_facet_revision','narrative_thread_revision'));
