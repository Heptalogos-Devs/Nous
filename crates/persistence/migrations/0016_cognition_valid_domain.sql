-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
ALTER TABLE memory_revisions ADD COLUMN valid_time_domain_ref text NOT NULL DEFAULT 'time:real' CHECK(length(valid_time_domain_ref) BETWEEN 1 AND 256);
ALTER TABLE cognitive_schema_revisions ADD COLUMN valid_time_domain_ref text NOT NULL DEFAULT 'time:real' CHECK(length(valid_time_domain_ref) BETWEEN 1 AND 256);
ALTER TABLE association_evidence ADD COLUMN valid_time_domain_ref text NOT NULL DEFAULT 'time:real' CHECK(length(valid_time_domain_ref) BETWEEN 1 AND 256);
UPDATE self_revisions SET content=jsonb_set(content,'{value,valid_time_domain_ref}','"time:real"'::jsonb);
UPDATE social_revisions SET content=jsonb_set(content,'{value,valid_time_domain_ref}','"time:real"'::jsonb);
UPDATE motivation_revisions SET content=jsonb_set(content,'{value,valid_time_domain_ref}','"time:real"'::jsonb);

-- Upgrade generated canonical messages at explicit owner envelope boundaries.
-- Neither model proposals nor arbitrary Struct/context fields are traversed.
CREATE FUNCTION pg_temp.domain_canonical(v jsonb, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE key text; path text[]; value jsonb; stamp jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 IF v ? 'formedStamp' THEN v=(v-'formedStamp')||jsonb_build_object('formedAt',v->'formedStamp'); END IF;
 IF jsonb_typeof(v->'formedAt')='string' THEN
  v=jsonb_set(v,'{formedAt}',jsonb_build_object('timeDomainRef','time:real','at',v->'formedAt','recordedAt',COALESCE(v->'recordedAt',to_jsonb(recorded))));
 END IF;
 FOREACH key IN ARRAY ARRAY['validTime','occurredTime','content,validTime','input,validTime','applicabilityScope,validTime','schema,applicabilityScope,validTime','content,facet,validTime','content,narrative,validTime','content,relation,validTime','content,convention,validTime','content,desire,validTime','content,concern,validTime','content,desiredCondition,validTime'] LOOP
  path=string_to_array(key,','); value=v#>path;
  IF jsonb_typeof(value)='object' AND NOT (value ? 'timeDomainRef' OR value ? 'extent') THEN
   v=jsonb_set(v,path,jsonb_build_object('timeDomainRef','time:real','extent',value));
  END IF;
 END LOOP;
 IF v ? 'currentRevision' THEN v=jsonb_set(v,'{currentRevision}',pg_temp.domain_canonical(v->'currentRevision',recorded)); END IF;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.domain_maintenance_plan(v jsonb, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE key text; entries jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 FOREACH key IN ARRAY ARRAY['episodes','members'] LOOP
  IF jsonb_typeof(v->key)='array' THEN
   SELECT COALESCE(jsonb_agg(pg_temp.domain_canonical(e,recorded) ORDER BY n),'[]'::jsonb) INTO entries FROM jsonb_array_elements(v->key) WITH ORDINALITY x(e,n);
   v=jsonb_set(v,ARRAY[key],entries);
  END IF;
 END LOOP;
 IF v ? 'journal' THEN v=jsonb_set(v,'{journal}',pg_temp.domain_canonical(v->'journal',recorded)); END IF;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.domain_reflection_proposal(v jsonb, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE key text; entries jsonb; action jsonb; actions jsonb;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 FOREACH key IN ARRAY ARRAY['currentSelf','currentSocial','currentMemory','currentMotivation'] LOOP
  IF jsonb_typeof(v->'plan'->key)='array' THEN
   SELECT COALESCE(jsonb_agg(jsonb_build_array(e->0,pg_temp.domain_canonical(e->1,recorded)) ORDER BY n),'[]'::jsonb) INTO entries FROM jsonb_array_elements(v->'plan'->key) WITH ORDINALITY x(e,n);
   v=jsonb_set(v,ARRAY['plan',key],entries);
  END IF;
 END LOOP;
 IF v->'plan' ? 'batch' THEN v=jsonb_set(v,'{plan,batch}',pg_temp.domain_maintenance_plan(v#>'{plan,batch}',recorded)); END IF;
 IF v ? 'paid' THEN v=jsonb_set(v,'{paid}',pg_temp.domain_reflection_proposal(v->'paid',recorded)); END IF;
 IF jsonb_typeof(v->'actions')='array' THEN
  actions='[]'::jsonb;
  FOR action IN SELECT * FROM jsonb_array_elements(v->'actions') LOOP
   FOREACH key IN ARRAY ARRAY['request','record'] LOOP
    IF action ? key THEN action=jsonb_set(action,ARRAY[key],pg_temp.domain_canonical(action->key,recorded)); END IF;
   END LOOP;
   actions=actions||jsonb_build_array(action);
  END LOOP;
  v=jsonb_set(v,'{actions}',actions);
 END IF;
 RETURN v;
END $$;
CREATE FUNCTION pg_temp.domain_reflection_result(v jsonb, recorded timestamptz) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE actions jsonb='[]'::jsonb; action jsonb; key text;
BEGIN
 IF jsonb_typeof(v->'actions') IS DISTINCT FROM 'array' THEN RETURN v; END IF;
 FOR action IN SELECT * FROM jsonb_array_elements(v->'actions') LOOP
  FOREACH key IN ARRAY ARRAY['selfRecord','socialRecord','memoryRecord','motivationRecord'] LOOP
   IF action ? key THEN action=jsonb_set(action,ARRAY[key],pg_temp.domain_canonical(action->key,recorded)); END IF;
  END LOOP;
  actions=actions||jsonb_build_array(action);
 END LOOP;
 RETURN jsonb_set(v,'{actions}',actions);
END $$;
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload}',pg_temp.domain_reflection_proposal(proposal->'payload',created_at)) WHERE owner='subject_reflection' AND proposal IS NOT NULL;
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,result}',pg_temp.domain_reflection_result(outcome#>'{payload,result}',created_at)) WHERE owner='subject_reflection' AND outcome->'payload' ? 'result';
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,proposal}',pg_temp.domain_reflection_proposal(outcome#>'{payload,proposal}',created_at)) WHERE owner='subject_reflection' AND outcome->'payload' ? 'proposal';
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload,request}',pg_temp.domain_canonical(proposal#>'{payload,request}',created_at)) WHERE owner='memory' AND proposal->'payload' ? 'request';
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload,input}',pg_temp.domain_maintenance_plan(proposal#>'{payload,input}',created_at)) WHERE owner='memory' AND proposal->'payload' ? 'input';
