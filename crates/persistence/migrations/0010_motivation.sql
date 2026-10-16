-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

ALTER TABLE subject_capabilities ADD COLUMN motivation boolean NOT NULL DEFAULT false;
CREATE TABLE motivation_objects (
 object_id uuid PRIMARY KEY,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 family text NOT NULL CHECK(family IN ('desire','concern','desired_condition')),
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
CREATE TABLE motivation_revisions (
 revision_id uuid PRIMARY KEY,
 object_id uuid NOT NULL REFERENCES motivation_objects(object_id) ON DELETE CASCADE,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 revision_no integer NOT NULL CHECK(revision_no>0),
 parent_revision_id uuid REFERENCES motivation_revisions(revision_id),
 content jsonb NOT NULL CHECK(jsonb_typeof(content)='object'),
 reason text NOT NULL,
 formed_at timestamptz NOT NULL,
 recorded_at timestamptz NOT NULL,
 UNIQUE(object_id,revision_no)
);
ALTER TABLE motivation_objects ADD CONSTRAINT motivation_current_revision_fk FOREIGN KEY(current_revision_id)
 REFERENCES motivation_revisions(revision_id) DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE motivation_evidence (
 revision_id uuid NOT NULL REFERENCES motivation_revisions(revision_id) ON DELETE CASCADE,
 role text NOT NULL CHECK(role IN ('support','counterexample','boundary_case')),
 basis_key text NOT NULL,
 basis jsonb NOT NULL,
 recorded_at timestamptz NOT NULL,
 PRIMARY KEY(revision_id,role,basis_key)
);
CREATE INDEX motivation_subject_current ON motivation_objects(subject_id,family,object_id);
CREATE INDEX motivation_revision_history ON motivation_revisions(object_id,revision_no);
CREATE TABLE motivation_dependencies (
 revision_id uuid NOT NULL REFERENCES motivation_revisions(revision_id) ON DELETE CASCADE,
 ref_kind text NOT NULL,
 ref_value text NOT NULL,
 recorded_at timestamptz NOT NULL,
 PRIMARY KEY(revision_id,ref_kind,ref_value)
);
CREATE INDEX motivation_dependencies_source ON motivation_dependencies(ref_kind,ref_value);

ALTER TABLE cognitive_use_events DROP CONSTRAINT cognitive_use_events_ref_kind_check;
ALTER TABLE cognitive_use_events ADD CONSTRAINT cognitive_use_events_ref_kind_check
 CHECK(ref_kind IN ('memory_revision','cognitive_schema_revision','episode_revision','journal_revision','self_facet_revision','narrative_thread_revision','social_relation_revision','language_convention_revision','desire_revision','concern_revision','desired_condition_revision'));
CREATE TRIGGER motivation_state_history AFTER INSERT OR UPDATE ON motivation_objects FOR EACH ROW
 EXECUTE FUNCTION capture_authority_object_state('motivation','object_id');

-- Semantic time is independent of real Authority recorded_at.
ALTER TABLE motivation_revisions ADD COLUMN formed_stamp jsonb NOT NULL;
CREATE TABLE motivation_evaluations (
 evaluation_id uuid PRIMARY KEY,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 condition_revision_id uuid NOT NULL,
 operation_id uuid NOT NULL,
 request_digest text NOT NULL,
 document jsonb NOT NULL,
 recorded_at timestamptz NOT NULL,
 UNIQUE(subject_id,operation_id)
);
-- Exact metadata edges for the single shared SourceRead SQL closure.
CREATE FUNCTION motivation_source_read_edges(p_subject uuid,p_kind text,p_ref text)
RETURNS TABLE(kind text,value text) LANGUAGE sql STABLE AS $$
 SELECT d.ref_kind,d.ref_value FROM motivation_objects o
 JOIN motivation_revisions r USING(object_id) JOIN motivation_dependencies d USING(revision_id)
 JOIN subject_capabilities c ON c.subject_id=o.subject_id
 WHERE o.subject_id=p_subject AND c.motivation AND o.acceptance_state='accepted'
 AND o.integrity_state='valid' AND o.suppression_state='normal'
 AND ((p_kind=o.family||'_revision' AND r.revision_id::text=p_ref)
 OR (p_kind=o.family AND o.object_id::text=p_ref AND r.revision_id=o.current_revision_id))
$$;
