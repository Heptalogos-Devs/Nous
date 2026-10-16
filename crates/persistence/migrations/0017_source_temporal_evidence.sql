-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- Source facts retain their actual domain and contact pair. Permission remains
-- with the existing source_read_allowed predicate at each consumer's read view.
DROP FUNCTION material_occurrence_facts(uuid,timestamptz,uuid[]);
CREATE FUNCTION material_occurrence_facts(owner_subject uuid, cutoff timestamptz, binding_ids uuid[])
RETURNS TABLE (occurrence_id uuid, source_class text, actor_entity_ref text,
 occurred_time_kind text, occurred_time_start timestamptz, occurred_time_end timestamptz,
 occurred_time_domain_ref text, observed_at timestamptz, observed_stamp jsonb, recorded_at timestamptz, entities text[])
LANGUAGE SQL STABLE AS $$
 SELECT o.occurrence_id,o.source_class,o.actor_entity_ref,
 o.occurred_time_kind,o.occurred_time_start,o.occurred_time_end,o.occurred_time_domain_ref,o.observed_at,o.observed_stamp,o.created_at,
 ARRAY(SELECT DISTINCT b.entity_ref FROM entity_mentions m
 CROSS JOIN LATERAL (SELECT entity_ref,binding_state FROM entity_binding_revisions WHERE mention_id=m.mention_id
 AND (binding_ids IS NULL OR binding_revision_id=ANY(binding_ids)) ORDER BY revision_no DESC LIMIT 1) b
 WHERE m.subject_id=owner_subject AND m.occurrence_id=o.occurrence_id AND b.binding_state='bound' AND b.entity_ref IS NOT NULL)
 FROM observation_occurrences o WHERE o.subject_id=owner_subject AND (cutoff IS NULL OR o.created_at<=cutoff)
$$;

-- Old independent aggregate axes cannot establish which observation witnessed
-- both. Rebuild only from the snapshot's already-declared evidence IDs, never
-- from an Artifact match or arbitrary conversation history.
CREATE FUNCTION pg_temp.source_time_record(v jsonb, owner_subject uuid, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE sources jsonb; key text; path text[]; value jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 IF v ? 'temporalEvidence' AND NOT (v->'temporalEvidence' ? 'sources') THEN
  SELECT COALESCE(jsonb_agg(jsonb_build_object(
   'occurrenceId',o.occurrence_id,
   'occurred',jsonb_build_object('timeDomainRef',o.occurred_time_domain_ref,'extent',CASE o.occurred_time_kind WHEN 'instant' THEN jsonb_build_object('instant',o.occurred_time_start) WHEN 'interval' THEN jsonb_build_object('interval',jsonb_strip_nulls(jsonb_build_object('start',o.occurred_time_start,'end',o.occurred_time_end))) ELSE '{}'::jsonb END),
   'observed',jsonb_build_object('timeDomainRef',o.observed_stamp->'time_domain_ref','at',o.observed_stamp->'at','recordedAt',o.observed_stamp->'recorded_at'),
   'observedAt',o.observed_at,'recordedAt',o.created_at) ORDER BY o.observed_at,o.occurrence_id),'[]'::jsonb)
  INTO sources FROM observation_occurrences o
  WHERE o.subject_id=owner_subject AND EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(v->'basis','[]'::jsonb)) b WHERE b#>>'{evidence,occurrenceId}'=o.occurrence_id::text);
  v=jsonb_set(v,'{temporalEvidence}',jsonb_build_object('sources',sources));
 END IF;
 -- Canonical private maintenance candidates changed together with public records.
 IF jsonb_typeof(v->'formedAt')='string' THEN
  v=jsonb_set(v,'{formedAt}',jsonb_build_object('timeDomainRef','time:real','at',v->'formedAt','recordedAt',COALESCE(v->'recordedAt',to_jsonb(recorded))));
 END IF;
 FOREACH key IN ARRAY ARRAY['validTime','occurredTime'] LOOP
  value=v->key;
  IF jsonb_typeof(value)='object' AND NOT (value ? 'timeDomainRef' OR value ? 'extent') THEN
   v=jsonb_set(v,ARRAY[key],jsonb_build_object('timeDomainRef','time:real','extent',value));
  END IF;
 END LOOP;
 IF v ? 'currentRevision' THEN v=jsonb_set(v,'{currentRevision}',pg_temp.source_time_record(v->'currentRevision',owner_subject,recorded)); END IF;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.source_time_plan(v jsonb, owner_subject uuid, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE key text; entries jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 FOREACH key IN ARRAY ARRAY['members','episodes','candidates'] LOOP
  IF jsonb_typeof(v->key)='array' THEN
   SELECT COALESCE(jsonb_agg(pg_temp.source_time_record(e,owner_subject,recorded) ORDER BY n),'[]'::jsonb) INTO entries FROM jsonb_array_elements(v->key) WITH ORDINALITY x(e,n);
   v=jsonb_set(v,ARRAY[key],entries);
  END IF;
 END LOOP;
 IF v ? 'journal' THEN v=jsonb_set(v,'{journal}',pg_temp.source_time_record(v->'journal',owner_subject,recorded)); END IF;
 IF jsonb_typeof(v->'cognitiveNow')='string' THEN v=jsonb_set(v,'{cognitiveNow}',jsonb_build_object('timeDomainRef','time:real','at',v->'cognitiveNow','recordedAt',recorded)); END IF;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.source_time_proposal(v jsonb, owner_subject uuid, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE key text; entries jsonb; action jsonb; actions jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 FOREACH key IN ARRAY ARRAY['currentSelf','currentSocial','currentMemory','currentMotivation'] LOOP
  IF jsonb_typeof(v->'plan'->key)='array' THEN
   SELECT COALESCE(jsonb_agg(jsonb_build_array(e->0,pg_temp.source_time_record(e->1,owner_subject,recorded)) ORDER BY n),'[]'::jsonb) INTO entries FROM jsonb_array_elements(v->'plan'->key) WITH ORDINALITY x(e,n);
   v=jsonb_set(v,ARRAY['plan',key],entries);
  END IF;
 END LOOP;
 IF v->'plan' ? 'batch' THEN v=jsonb_set(v,'{plan,batch}',pg_temp.source_time_plan(v#>'{plan,batch}',owner_subject,recorded)); END IF;
 IF v ? 'paid' THEN v=jsonb_set(v,'{paid}',pg_temp.source_time_proposal(v->'paid',owner_subject,recorded)); END IF;
 IF jsonb_typeof(v->'actions')='array' THEN
  actions='[]'::jsonb;
  FOR action IN SELECT * FROM jsonb_array_elements(v->'actions') LOOP
   IF action ? 'record' THEN action=jsonb_set(action,'{record}',pg_temp.source_time_record(action->'record',owner_subject,recorded)); END IF;
   actions=actions||jsonb_build_array(action);
  END LOOP;
  v=jsonb_set(v,'{actions}',actions);
 END IF;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.source_time_outcome(v jsonb, owner_subject uuid, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE actions jsonb='[]'::jsonb; action jsonb; key text;
BEGIN
 IF jsonb_typeof(v->'actions') IS DISTINCT FROM 'array' THEN RETURN v; END IF;
 FOR action IN SELECT * FROM jsonb_array_elements(v->'actions') LOOP
  FOREACH key IN ARRAY ARRAY['selfRecord','socialRecord','memoryRecord','motivationRecord'] LOOP
   IF action ? key THEN action=jsonb_set(action,ARRAY[key],pg_temp.source_time_record(action->key,owner_subject,recorded)); END IF;
  END LOOP;
  actions=actions||jsonb_build_array(action);
 END LOOP;
 RETURN jsonb_set(v,'{actions}',actions);
END $$;
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload}',pg_temp.source_time_proposal(proposal->'payload',subject_id,created_at)) WHERE owner='subject_reflection' AND proposal IS NOT NULL;
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,result}',pg_temp.source_time_outcome(outcome#>'{payload,result}',subject_id,created_at)) WHERE owner='subject_reflection' AND outcome->'payload' ? 'result';
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,proposal}',pg_temp.source_time_proposal(outcome#>'{payload,proposal}',subject_id,created_at)) WHERE owner='subject_reflection' AND outcome->'payload' ? 'proposal';
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload,input}',pg_temp.source_time_plan(proposal#>'{payload,input}',subject_id,created_at)) WHERE owner='memory' AND proposal->'payload' ? 'input';
