-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- One-way owner metadata upgrade. Provider raw output is deliberately untouched.
UPDATE model_results SET context=jsonb_set(context,'{cognitive_formed_at}',
 jsonb_build_object('time_domain_ref','time:real','at',context->'cognitive_formed_at','recorded_at',created_at))
WHERE jsonb_typeof(context->'cognitive_formed_at')='string';

CREATE FUNCTION authority_formation_domain() RETURNS text LANGUAGE SQL STABLE AS $$
 SELECT COALESCE(NULLIF(current_setting('nous.formed_stamp',true),'')::jsonb->>'time_domain_ref','time:real')
$$;
ALTER TABLE memory_revisions ADD COLUMN formed_time_domain_ref text NOT NULL DEFAULT authority_formation_domain();
ALTER TABLE cognitive_schema_revisions ADD COLUMN formed_time_domain_ref text NOT NULL DEFAULT authority_formation_domain();
ALTER TABLE episode_revisions ADD COLUMN formed_time_domain_ref text NOT NULL DEFAULT authority_formation_domain();
ALTER TABLE journal_revisions ADD COLUMN formed_time_domain_ref text NOT NULL DEFAULT authority_formation_domain();
ALTER TABLE self_revisions ADD COLUMN formed_time_domain_ref text NOT NULL DEFAULT authority_formation_domain();
ALTER TABLE social_revisions ADD COLUMN formed_time_domain_ref text NOT NULL DEFAULT authority_formation_domain();

-- These are canonical owner records inside recorded reflection plans/receipts.
-- No traversal of prompts, generated proposal text, Material or native history.
CREATE FUNCTION pg_temp.upgrade_formation_record(v jsonb, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE stamp jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 IF v ? 'formedStamp' THEN RETURN (v-'formedStamp') || jsonb_build_object('formedAt',v->'formedStamp'); END IF;
 IF jsonb_typeof(v->'formedAt')='string' THEN
  stamp=jsonb_build_object('timeDomainRef','time:real','at',v->'formedAt','recordedAt',COALESCE(v->'recordedAt',to_jsonb(recorded)));
  RETURN jsonb_set(v,'{formedAt}',stamp);
 END IF;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.upgrade_reflection_proposal(v jsonb, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE name text; entries jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 v=pg_temp.upgrade_formation_record(v,recorded);
 FOREACH name IN ARRAY ARRAY['currentSelf','currentSocial','currentMemory','currentMotivation'] LOOP
  IF jsonb_typeof(v->'plan'->name)='array' THEN
   SELECT COALESCE(jsonb_agg(jsonb_build_array(e->0,pg_temp.upgrade_formation_record(e->1,recorded)) ORDER BY n),'[]'::jsonb)
   INTO entries FROM jsonb_array_elements(v->'plan'->name) WITH ORDINALITY x(e,n);
   v=jsonb_set(v,ARRAY['plan',name],entries);
  END IF;
 END LOOP;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.upgrade_reflection_result(v jsonb, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE actions jsonb; action jsonb; name text;
BEGIN
 IF jsonb_typeof(v->'actions') IS DISTINCT FROM 'array' THEN RETURN v; END IF;
 actions='[]'::jsonb;
 FOR action IN SELECT * FROM jsonb_array_elements(v->'actions') LOOP
  FOREACH name IN ARRAY ARRAY['selfRecord','socialRecord','memoryRecord','motivationRecord'] LOOP
   IF action ? name THEN action=jsonb_set(action,ARRAY[name],pg_temp.upgrade_formation_record(action->name,recorded)); END IF;
  END LOOP;
  actions=actions||jsonb_build_array(action);
 END LOOP;
 RETURN jsonb_set(v,'{actions}',actions);
END $$;
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload}',pg_temp.upgrade_reflection_proposal(proposal->'payload',created_at))
 WHERE owner='subject_reflection' AND proposal IS NOT NULL AND proposal->'payload' ? 'plan';
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload,paid}',pg_temp.upgrade_reflection_proposal(proposal->'payload'->'paid',created_at))
 WHERE owner='subject_reflection' AND proposal->'payload' ? 'paid';
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,result}',pg_temp.upgrade_reflection_result(outcome->'payload'->'result',created_at))
 WHERE owner='subject_reflection' AND outcome->'payload' ? 'result';
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,proposal}',pg_temp.upgrade_reflection_proposal(outcome->'payload'->'proposal',created_at))
 WHERE owner='subject_reflection' AND outcome->'payload' ? 'proposal';
