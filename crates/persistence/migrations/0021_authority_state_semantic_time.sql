-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- One state-history owner retains both semantic commit coordinate and real
-- recording order. Old rows are real; no past world mapping is inferred.
CREATE FUNCTION authority_semantic_stamp(owner_subject uuid DEFAULT NULL) RETURNS jsonb LANGUAGE SQL STABLE AS $$
 SELECT COALESCE(CASE WHEN NULLIF(current_setting('nous.semantic_subject',true),'')=owner_subject::text THEN NULLIF(current_setting('nous.semantic_stamp',true),'')::jsonb END,
 jsonb_build_object('time_domain_ref','time:real','at',authority_recording_time(),'recorded_at',authority_recording_time()))
$$;
ALTER TABLE authority_object_states ADD COLUMN semantic_stamp jsonb;
UPDATE authority_object_states SET semantic_stamp=jsonb_build_object('time_domain_ref','time:real','at',recorded_at,'recorded_at',recorded_at);
ALTER TABLE authority_object_states ALTER COLUMN semantic_stamp SET NOT NULL,
 ALTER COLUMN semantic_stamp SET DEFAULT authority_semantic_stamp();

CREATE OR REPLACE FUNCTION capture_authority_object_state() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
 payload jsonb := to_jsonb(NEW);
 captured timestamptz := authority_recording_time();
BEGIN
 IF TG_OP='INSERT' OR to_jsonb(OLD) IS DISTINCT FROM payload THEN
  INSERT INTO authority_object_states(subject_id,object_kind,object_ref,state,recorded_at,semantic_stamp)
  VALUES(NEW.subject_id,TG_ARGV[0],payload->>TG_ARGV[1],payload,captured,authority_semantic_stamp(NEW.subject_id));
 END IF;
 RETURN NEW;
END $$;
