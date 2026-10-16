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

-- Runtime state coverage starts with actual migration-time snapshots, never before them.
CREATE TRIGGER work_context_state_history AFTER INSERT OR UPDATE ON work_contexts FOR EACH ROW
 EXECUTE FUNCTION capture_authority_object_state('work_context','work_context_id');
INSERT INTO authority_object_states(subject_id,object_kind,object_ref,state,recorded_at)
 SELECT subject_id,'work_context',work_context_id::text,to_jsonb(w),clock_timestamp() FROM work_contexts w;

-- A condition's read selectors are not beliefs supporting its semantic validity.
CREATE TABLE motivation_criterion_refs (
 revision_id uuid NOT NULL REFERENCES motivation_revisions(revision_id) ON DELETE CASCADE,
 ref_kind text NOT NULL,ref_value text NOT NULL,
 PRIMARY KEY(revision_id,ref_kind,ref_value)
);
-- Fixed shared-closure adapter. 0011 supplies the single basis/locator traversal;
-- PL/pgSQL resolves that function only when this adapter is called after migration.
CREATE FUNCTION motivation_source_read_edges_with_mode(p_subject uuid,p_kind text,p_ref text,p_metadata_only boolean)
RETURNS TABLE(kind text,value text,metadata_edge boolean) LANGUAGE plpgsql STABLE AS $$
DECLARE selected uuid;
BEGIN
 SELECT r.revision_id INTO selected FROM motivation_objects o JOIN motivation_revisions r USING(object_id)
 JOIN subject_capabilities c ON c.subject_id=o.subject_id
 WHERE o.subject_id=p_subject AND c.motivation
 AND (p_metadata_only OR (o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'))
 AND ((p_kind=o.family||'_revision' AND r.revision_id::text=p_ref)
 OR (p_kind=o.family AND o.object_id::text=p_ref AND r.revision_id=o.current_revision_id));
 IF selected IS NULL THEN RETURN; END IF;
 RETURN QUERY
 SELECT edges.kind,edges.value,false FROM motivation_revisions r
 CROSS JOIN LATERAL jsonb_array_elements(r.content#>'{value,basis}') b
 CROSS JOIN LATERAL basis_source_read_edges(b) edges WHERE r.revision_id=selected
 UNION ALL
 SELECT edges.kind,edges.value,false FROM motivation_evidence e
 CROSS JOIN LATERAL basis_source_read_edges(e.basis) edges WHERE e.revision_id=selected
 UNION ALL
 SELECT l->'target_revision'->>'kind',l->'target_revision'->>'id',false FROM motivation_revisions r
 CROSS JOIN LATERAL jsonb_array_elements(r.content#>'{value,links}') l WHERE r.revision_id=selected
 UNION ALL
 SELECT selectors.ref_kind,selectors.ref_value,true FROM motivation_criterion_refs selectors WHERE selectors.revision_id=selected;
END $$;
