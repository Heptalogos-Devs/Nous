-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- One-time typed representation upgrade. Paid output, captured prompt and IDs stay immutable.
CREATE FUNCTION pg_temp.formal_purposes(v jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
 SELECT COALESCE(jsonb_agg(to_jsonb(CASE e #>> '{}' WHEN 'deliberation' THEN 'subject_deliberation' WHEN 'expression' THEN 'subject_rendering' WHEN 'worker' THEN 'professional_execution' ELSE e #>> '{}' END) ORDER BY n),'[]'::jsonb)
 FROM jsonb_array_elements(v) WITH ORDINALITY x(e,n)
$$;
CREATE FUNCTION pg_temp.formal_scope(v jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
 SELECT CASE WHEN jsonb_typeof(v->'purposes')='array' THEN jsonb_set(v,'{purposes}',pg_temp.formal_purposes(v->'purposes')) ELSE v END
$$;
CREATE FUNCTION pg_temp.formal_self_message(v jsonb) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE path text[]; key text;
BEGIN
 FOREACH key IN ARRAY ARRAY['content,facet,scope','facet,scope'] LOOP
  path=string_to_array(key,',');
  IF v#>path IS NOT NULL THEN v=jsonb_set(v,path,pg_temp.formal_scope(v#>path)); END IF;
 END LOOP;
 RETURN v;
END $$;
-- Persistence finishes these exact receipt hashes with Core's BLAKE3 canonical encoder
-- before opening the database to owners. The queue survives an interrupted migration.
CREATE TABLE purpose_migration_receipts AS
 SELECT m.subject_id,m.operation_id,m.operation_kind,m.request_digest,m.result_epoch,
 r.object_id,r.revision_id,r.parent_revision_id,r.content AS old_content,r.reason,
 ARRAY(SELECT DISTINCT a #>> '{proposal,reason}' FROM model_results p
 CROSS JOIN LATERAL jsonb_array_elements(COALESCE(p.proposal#>'{payload,output,actions}',p.outcome#>'{payload,proposal,output,actions}',p.proposal#>'{payload,paid,output,actions}','[]'::jsonb)) a
 WHERE p.subject_id=m.subject_id AND p.owner='subject_reflection' AND a->>'domain'='self') AS paid_reasons
 FROM mutation_receipts m JOIN self_revisions r ON r.revision_id=m.result_revision
 WHERE m.operation_kind IN ('self.adopt','self.revise')
 AND (r.content#>'{value,scope,purposes}') ?| ARRAY['deliberation','expression','worker'];
UPDATE self_revisions SET content=jsonb_set(content,'{value,scope}',pg_temp.formal_scope(content#>'{value,scope}'))
 WHERE content#>'{value,scope}' IS NOT NULL;
CREATE FUNCTION pg_temp.formal_reflection(v jsonb) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE entries jsonb; actions jsonb='[]'::jsonb; a jsonb; key text;
BEGIN
 IF jsonb_typeof(v) IS DISTINCT FROM 'object' THEN RETURN v; END IF;
 IF v ? 'output' THEN
  FOR a IN SELECT * FROM jsonb_array_elements(v#>'{output,actions}') LOOP
   IF a->>'domain'='self' AND a#>'{proposal,content,scope}' IS NOT NULL THEN
    a=jsonb_set(a,'{proposal,content,scope}',pg_temp.formal_scope(a#>'{proposal,content,scope}'));
   END IF;
   actions=actions||jsonb_build_array(a);
  END LOOP;
  v=v||jsonb_build_object('preparation',jsonb_set(v->'output','{actions}',actions));
 END IF;
 IF jsonb_typeof(v#>'{plan,currentSelf}')='array' THEN
  SELECT COALESCE(jsonb_agg(jsonb_build_array(e->0,pg_temp.formal_self_message(e->1)) ORDER BY n),'[]'::jsonb) INTO entries FROM jsonb_array_elements(v#>'{plan,currentSelf}') WITH ORDINALITY x(e,n);
  v=jsonb_set(v,'{plan,currentSelf}',entries);
 END IF;
 IF jsonb_typeof(v#>'{plan,scopes}')='array' THEN
  SELECT COALESCE(jsonb_agg(jsonb_build_array(e->0,jsonb_set(e->1,'{self}',pg_temp.formal_scope(e#>'{1,self}'))) ORDER BY n),'[]'::jsonb) INTO entries FROM jsonb_array_elements(v#>'{plan,scopes}') WITH ORDINALITY x(e,n);
  v=jsonb_set(v,'{plan,scopes}',entries);
 END IF;
 IF v ? 'paid' THEN v=jsonb_set(v,'{paid}',pg_temp.formal_reflection(v->'paid')); END IF;
 IF jsonb_typeof(v->'actions')='array' THEN
  actions='[]'::jsonb;
  FOR a IN SELECT * FROM jsonb_array_elements(v->'actions') LOOP
   IF a->>'domain'='self' THEN
    FOREACH key IN ARRAY ARRAY['request','record'] LOOP
     IF a ? key THEN a=jsonb_set(a,ARRAY[key],pg_temp.formal_self_message(a->key)); END IF;
    END LOOP;
   END IF;
   actions=actions||jsonb_build_array(a);
  END LOOP;
  v=jsonb_set(v,'{actions}',actions);
 END IF;
 RETURN v;
END $$;
UPDATE model_results SET proposal=jsonb_set(proposal,'{payload}',pg_temp.formal_reflection(proposal->'payload')) WHERE owner='subject_reflection' AND proposal IS NOT NULL;
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,proposal}',pg_temp.formal_reflection(outcome#>'{payload,proposal}')) WHERE owner='subject_reflection' AND outcome#>'{payload,proposal}' IS NOT NULL;
-- Outcome SelfRecord snapshots are typed current representations, not model text.
CREATE FUNCTION pg_temp.formal_outcome(v jsonb) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE a jsonb; actions jsonb='[]'::jsonb;
BEGIN
 IF jsonb_typeof(v->'actions') IS DISTINCT FROM 'array' THEN RETURN v; END IF;
 FOR a IN SELECT * FROM jsonb_array_elements(v->'actions') LOOP
  IF a ? 'selfRecord' THEN a=jsonb_set(a,'{selfRecord}',pg_temp.formal_self_message(a->'selfRecord')); END IF;
  actions=actions||jsonb_build_array(a);
 END LOOP;
 RETURN jsonb_set(v,'{actions}',actions);
END $$;
UPDATE model_results SET outcome=jsonb_set(outcome,'{payload,result}',pg_temp.formal_outcome(outcome#>'{payload,result}')) WHERE owner='subject_reflection' AND outcome#>'{payload,result}' IS NOT NULL;
