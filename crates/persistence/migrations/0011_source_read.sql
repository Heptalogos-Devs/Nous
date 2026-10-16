-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- Read-only owner metadata functions; no new authorization truth/index.
CREATE FUNCTION material_lineage_read(uuid,text[],uuid[],timestamptz)
RETURNS TABLE(ordinality bigint,present boolean,derived_id uuid,source_class text,external_object_ref text,artifacts uuid[],roots text[])
LANGUAGE sql STABLE AS $lineage$

WITH RECURSIVE requested AS (SELECT kind,id,ordinality FROM unnest($2::text[],$3::uuid[]) WITH ORDINALITY AS r(kind,id,ordinality)),
resolved AS (
 SELECT r.*,o.source_class,o.external_object_ref,
 o.occurrence_id IS NOT NULL OR a.artifact_id IS NOT NULL OR sr.source_region_id IS NOT NULL OR d.derived_representation_id IS NOT NULL OR dr.derived_region_id IS NOT NULL AS present,
 coalesce(o.artifact_id,a.artifact_id,sr.artifact_id) AS artifact_id,coalesce(d.derived_representation_id,dr.derived_representation_id) AS derived_id
 FROM requested r
 LEFT JOIN observation_occurrences o ON r.kind='occurrence' AND o.subject_id=$1 AND o.occurrence_id=r.id AND ($4::timestamptz IS NULL OR o.created_at<=$4)
 LEFT JOIN artifacts a ON r.kind='artifact' AND a.subject_id=$1 AND a.artifact_id=r.id
 LEFT JOIN source_regions sr ON r.kind='source_region' AND sr.subject_id=$1 AND sr.source_region_id=r.id AND ($4::timestamptz IS NULL OR sr.created_at<=$4)
 LEFT JOIN derived_representations d ON r.kind='derived_representation' AND d.subject_id=$1 AND d.derived_representation_id=r.id
 LEFT JOIN derived_regions dr ON r.kind='derived_region' AND dr.subject_id=$1 AND dr.derived_region_id=r.id
), sources AS (
 SELECT ordinality,artifact_id FROM resolved WHERE artifact_id IS NOT NULL
 UNION SELECT r.ordinality,sr.artifact_id FROM resolved r CROSS JOIN LATERAL representation_source_regions($1,r.derived_id) roots JOIN source_regions sr USING(source_region_id)
 WHERE sr.subject_id=$1 AND ($4::timestamptz IS NULL OR sr.created_at<=$4)
), lineage AS (
 SELECT s.artifact_id AS origin,o.occurrence_id,o.artifact_id,o.source_class,o.external_object_ref FROM (SELECT DISTINCT artifact_id FROM sources) s
 JOIN observation_occurrences o ON o.subject_id=$1 AND o.artifact_id=s.artifact_id AND ($4::timestamptz IS NULL OR o.created_at<=$4)
 UNION SELECT p.origin,o.occurrence_id,o.artifact_id,o.source_class,o.external_object_ref FROM lineage p
 JOIN observation_occurrences o ON (o.artifact_id IS NOT NULL AND o.artifact_id=p.artifact_id) OR (o.external_object_ref IS NOT NULL AND o.external_object_ref=p.external_object_ref AND o.source_class=p.source_class)
 WHERE o.subject_id=$1 AND ($4::timestamptz IS NULL OR o.created_at<=$4)
), external_roots AS (
 SELECT origin,MIN('external:'||source_class||':'||external_object_ref) AS root FROM lineage WHERE external_object_ref IS NOT NULL GROUP BY origin
)
SELECT r.ordinality,r.present,r.derived_id,r.source_class,r.external_object_ref,
 ARRAY(SELECT DISTINCT s.artifact_id FROM sources s WHERE s.ordinality=r.ordinality) AS artifacts,
 ARRAY(SELECT DISTINCT coalesce(e.root,'artifact:'||s.artifact_id) FROM sources s LEFT JOIN external_roots e ON e.origin=s.artifact_id WHERE s.ordinality=r.ordinality) AS roots
FROM resolved r

$lineage$;

CREATE FUNCTION basis_source_read_edges(basis jsonb) RETURNS TABLE(kind text,value text) LANGUAGE sql IMMUTABLE AS $basis$
 SELECT CASE basis->>'kind' WHEN 'evidence' THEN 'occurrence' WHEN 'seed' THEN 'cognitive_seed_version' ELSE basis->'value'->'target_revision'->>'kind' END,
 CASE basis->>'kind' WHEN 'evidence' THEN basis->'value'->>'occurrence_id' WHEN 'seed' THEN basis->'value'->>'seed_version_id' ELSE basis->'value'->'target_revision'->>'id' END
 UNION ALL
 SELECT 'evidence_locator',jsonb_build_object('occurrence',basis->'value'->>'occurrence_id','kind',CASE WHEN basis->'value'->'locator' ? 'SourceRegion' THEN 'source_region' WHEN basis->'value'->'locator' ? 'DerivedRepresentation' THEN 'derived_representation' ELSE 'derived_region' END,'id',COALESCE(basis->'value'->'locator'->>'SourceRegion',basis->'value'->'locator'->>'DerivedRepresentation',basis->'value'->'locator'->>'DerivedRegion'))::text
 WHERE basis->>'kind'='evidence' AND jsonb_typeof(basis->'value'->'locator')='object'
$basis$;

CREATE FUNCTION source_read_edges(owner_subject uuid, source_kind text, source_value text, metadata_only boolean DEFAULT false)
RETURNS TABLE(kind text,value text) LANGUAGE plpgsql STABLE AS $edges$
DECLARE source_id uuid;
BEGIN
 IF source_kind IN ('occurrence','artifact','source_region','derived_representation','derived_region','evidence_locator') THEN RETURN; END IF;
 source_id := source_value::uuid;
 IF source_kind IN ('memory','memory_revision','episode','episode_revision','journal','journal_revision','cognitive_schema','cognitive_schema_revision','association','tag') AND NOT EXISTS(SELECT 1 FROM subject_capabilities c WHERE c.subject_id=owner_subject AND c.memory) THEN RETURN; END IF;
 IF source_kind IN ('self_facet','self_facet_revision','narrative_thread','narrative_thread_revision') AND NOT EXISTS(SELECT 1 FROM subject_capabilities c WHERE c.subject_id=owner_subject AND c.self_cognition) THEN RETURN; END IF;
 IF source_kind IN ('social_relation','social_relation_revision','language_convention','language_convention_revision') AND NOT EXISTS(SELECT 1 FROM subject_capabilities c WHERE c.subject_id=owner_subject AND c.social_cognition) THEN RETURN; END IF;

 IF source_kind IN ('memory_revision') AND NOT metadata_only AND NOT EXISTS(SELECT 1 FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=owner_subject AND r.memory_revision_id=source_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal') THEN RETURN; END IF;
 IF source_kind IN ('cognitive_schema_revision') AND NOT metadata_only AND NOT EXISTS(SELECT 1 FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=owner_subject AND r.schema_revision_id=source_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal') THEN RETURN; END IF;
 IF source_kind IN ('episode_revision') AND NOT metadata_only AND NOT EXISTS(SELECT 1 FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=owner_subject AND r.episode_revision_id=source_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal') THEN RETURN; END IF;
 IF source_kind IN ('journal_revision') AND NOT metadata_only AND NOT EXISTS(SELECT 1 FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=owner_subject AND r.journal_revision_id=source_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal') THEN RETURN; END IF;
 IF source_kind IN ('self_facet_revision','narrative_thread_revision') AND NOT metadata_only AND NOT EXISTS(SELECT 1 FROM self_revisions r JOIN self_objects o USING(object_id) WHERE r.subject_id=owner_subject AND r.revision_id=source_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal') THEN RETURN; END IF;
 IF source_kind IN ('social_relation_revision','language_convention_revision') AND NOT metadata_only AND NOT EXISTS(SELECT 1 FROM social_revisions r JOIN social_objects o USING(object_id) WHERE r.subject_id=owner_subject AND r.revision_id=source_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal') THEN RETURN; END IF;
 CASE source_kind
 WHEN 'memory' THEN RETURN QUERY SELECT 'memory_revision'::text,o.current_revision_id::text FROM memory_objects o WHERE o.subject_id=owner_subject AND o.memory_id=source_id;
 WHEN 'episode' THEN RETURN QUERY SELECT 'episode_revision'::text,o.current_revision_id::text FROM episode_objects o WHERE o.subject_id=owner_subject AND o.episode_id=source_id;
 WHEN 'journal' THEN RETURN QUERY SELECT 'journal_revision'::text,o.current_revision_id::text FROM journal_objects o WHERE o.subject_id=owner_subject AND o.journal_id=source_id;
 WHEN 'cognitive_schema' THEN RETURN QUERY SELECT 'cognitive_schema_revision'::text,o.current_revision_id::text FROM cognitive_schemas o WHERE o.subject_id=owner_subject AND o.schema_id=source_id;
 WHEN 'self_facet','narrative_thread' THEN RETURN QUERY SELECT o.family||'_revision',o.current_revision_id::text FROM self_objects o WHERE o.subject_id=owner_subject AND o.object_id=source_id AND o.family=source_kind;
 WHEN 'social_relation','language_convention' THEN RETURN QUERY SELECT o.family||'_revision',o.current_revision_id::text FROM social_objects o WHERE o.subject_id=owner_subject AND o.object_id=source_id AND o.family=source_kind;
 WHEN 'self_facet_revision','narrative_thread_revision' THEN RETURN QUERY SELECT edge.kind,edge.value FROM self_revisions r JOIN self_objects o USING(object_id) CROSS JOIN LATERAL (
 SELECT entry AS basis FROM jsonb_array_elements(COALESCE(r.content#>'{value,basis}','[]'::jsonb)) entry
 UNION ALL SELECT e.basis FROM self_evidence e WHERE e.revision_id=r.revision_id
 UNION ALL SELECT b.entry FROM jsonb_array_elements(COALESCE(r.content#>'{value,exceptions}','[]'::jsonb)) ex CROSS JOIN LATERAL jsonb_array_elements(COALESCE(ex->'basis','[]'::jsonb)) b(entry)
) b CROSS JOIN LATERAL basis_source_read_edges(b.basis) edge WHERE r.subject_id=owner_subject AND r.revision_id=source_id AND o.family||'_revision'=source_kind UNION ALL SELECT entry->>'kind',entry->>'id' FROM self_revisions r JOIN self_objects o USING(object_id) CROSS JOIN LATERAL jsonb_array_elements(COALESCE(r.content#>'{value,source_refs}','[]'::jsonb)) entry WHERE r.subject_id=owner_subject AND r.revision_id=source_id AND o.family='narrative_thread';
 WHEN 'social_relation_revision','language_convention_revision' THEN RETURN QUERY SELECT edge.kind,edge.value FROM social_revisions r JOIN social_objects o USING(object_id) CROSS JOIN LATERAL (
 SELECT entry AS basis FROM jsonb_array_elements(COALESCE(r.content#>'{value,basis}','[]'::jsonb)) entry
 UNION ALL SELECT e.basis FROM social_evidence e WHERE e.revision_id=r.revision_id
) b CROSS JOIN LATERAL basis_source_read_edges(b.basis) edge WHERE r.subject_id=owner_subject AND r.revision_id=source_id AND o.family||'_revision'=source_kind;
 WHEN 'association' THEN RETURN QUERY SELECT CASE b.basis_kind WHEN 'evidence' THEN 'occurrence' ELSE b.basis_kind END,CASE b.basis_kind WHEN 'evidence' THEN b.occurrence_id::text ELSE b.basis_ref END FROM association_evidence_basis b JOIN association_evidence a USING(association_evidence_id) WHERE a.subject_id=owner_subject AND a.association_evidence_id=source_id AND a.revoked_at IS NULL UNION SELECT 'evidence_locator'::text,jsonb_build_object('occurrence',b.occurrence_id,'kind',CASE WHEN source_region_id IS NOT NULL THEN 'source_region' WHEN derived_representation_id IS NOT NULL THEN 'derived_representation' ELSE 'derived_region' END,'id',COALESCE(source_region_id,derived_representation_id,derived_region_id))::text FROM association_evidence_basis b JOIN association_evidence a USING(association_evidence_id) WHERE a.subject_id=owner_subject AND a.association_evidence_id=source_id AND a.revoked_at IS NULL AND num_nonnulls(source_region_id,derived_representation_id,derived_region_id)>0;
 WHEN 'cognitive_seed_version' THEN RETURN QUERY SELECT 'artifact'::text,s.artifact_id::text FROM cognitive_seed_versions s WHERE s.subject_id=owner_subject AND s.seed_version_id=source_id;
 WHEN 'memory_revision' THEN RETURN QUERY SELECT 'occurrence'::text, e.occurrence_id::text FROM memory_revision_evidence e JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=owner_subject AND r.memory_revision_id=source_id UNION SELECT 'evidence_locator'::text,jsonb_build_object('occurrence',e.occurrence_id,'kind',CASE WHEN source_region_id IS NOT NULL THEN 'source_region' WHEN derived_representation_id IS NOT NULL THEN 'derived_representation' ELSE 'derived_region' END,'id',COALESCE(source_region_id,derived_representation_id,derived_region_id))::text FROM memory_revision_evidence e JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=owner_subject AND r.memory_revision_id=source_id AND num_nonnulls(source_region_id,derived_representation_id,derived_region_id)>0 UNION SELECT d.target_ref_kind,d.target_ref FROM memory_revision_dependencies d JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=owner_subject AND r.memory_revision_id=source_id;
 WHEN 'cognitive_schema_revision' THEN RETURN QUERY SELECT CASE WHEN e.basis_kind='evidence' THEN 'occurrence' ELSE e.basis_kind END,CASE WHEN e.basis_kind='evidence' THEN e.occurrence_id::text ELSE e.basis_ref END FROM cognitive_schema_evidence_links e WHERE e.subject_id=owner_subject AND e.schema_revision_id=source_id AND e.revoked_at IS NULL UNION SELECT 'evidence_locator'::text,jsonb_build_object('occurrence',e.occurrence_id,'kind',CASE WHEN source_region_id IS NOT NULL THEN 'source_region' WHEN derived_representation_id IS NOT NULL THEN 'derived_representation' ELSE 'derived_region' END,'id',COALESCE(source_region_id,derived_representation_id,derived_region_id))::text FROM cognitive_schema_evidence_links e WHERE e.subject_id=owner_subject AND e.schema_revision_id=source_id AND e.revoked_at IS NULL AND num_nonnulls(source_region_id,derived_representation_id,derived_region_id)>0;
 WHEN 'episode_revision' THEN RETURN QUERY SELECT m.ref_kind,m.ref_value FROM episode_revision_members m JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=owner_subject AND r.episode_revision_id=source_id UNION SELECT CASE WHEN b.basis_kind='evidence' THEN 'occurrence' ELSE b.basis_kind END,CASE WHEN b.basis_kind='evidence' THEN b.occurrence_id::text ELSE b.basis_ref END FROM episode_revision_basis b JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=owner_subject AND r.episode_revision_id=source_id UNION SELECT 'evidence_locator'::text,jsonb_build_object('occurrence',b.occurrence_id,'kind',CASE WHEN source_region_id IS NOT NULL THEN 'source_region' WHEN derived_representation_id IS NOT NULL THEN 'derived_representation' ELSE 'derived_region' END,'id',COALESCE(source_region_id,derived_representation_id,derived_region_id))::text FROM episode_revision_basis b JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=owner_subject AND r.episode_revision_id=source_id AND num_nonnulls(source_region_id,derived_representation_id,derived_region_id)>0;
 WHEN 'journal_revision' THEN RETURN QUERY SELECT s.ref_kind,s.ref_value FROM journal_revision_sources s JOIN journal_revisions r USING(journal_revision_id) WHERE r.subject_id=owner_subject AND r.journal_revision_id=source_id;
 RETURN QUERY SELECT CASE p.basis->>'kind' WHEN 'evidence' THEN 'occurrence' WHEN 'seed' THEN 'cognitive_seed_version' ELSE p.basis->'value'->'target_revision'->>'kind' END, CASE p.basis->>'kind' WHEN 'evidence' THEN p.basis->'value'->>'occurrence_id' WHEN 'seed' THEN p.basis->'value'->>'seed_version_id' ELSE p.basis->'value'->'target_revision'->>'id' END FROM journal_point_basis p JOIN journal_revisions r USING(journal_revision_id) WHERE r.subject_id=owner_subject AND r.journal_revision_id=source_id;
 RETURN QUERY SELECT 'evidence_locator'::text,jsonb_build_object('occurrence',p.basis->'value'->>'occurrence_id','kind',CASE WHEN p.basis->'value'->'locator' ? 'SourceRegion' THEN 'source_region' WHEN p.basis->'value'->'locator' ? 'DerivedRepresentation' THEN 'derived_representation' ELSE 'derived_region' END,'id',COALESCE(p.basis->'value'->'locator'->>'SourceRegion',p.basis->'value'->'locator'->>'DerivedRepresentation',p.basis->'value'->'locator'->>'DerivedRegion'))::text FROM journal_point_basis p JOIN journal_revisions r USING(journal_revision_id) WHERE r.subject_id=owner_subject AND r.journal_revision_id=source_id AND p.basis->>'kind'='evidence' AND jsonb_typeof(p.basis->'value'->'locator')='object';
 ELSE RETURN;
 END CASE;
END $edges$;

-- Fixed owner graph hook; future owner migrations may extend only this dispatcher.
CREATE FUNCTION source_read_edges_with_mode(owner_subject uuid,source_kind text,source_value text,metadata_only boolean)
RETURNS TABLE(kind text,value text,metadata_edge boolean) LANGUAGE sql STABLE AS $$ SELECT e.kind,e.value,false FROM source_read_edges(owner_subject,source_kind,source_value,metadata_only) e $$;

-- Authorize actual provenance paths, separate from independence grouping of equal bytes.
CREATE FUNCTION material_source_read_allowed(owner_subject uuid,source_kind text,source_value text,origins text[],explicit_refs jsonb,actual_occurrence uuid DEFAULT NULL)
RETURNS boolean LANGUAGE plpgsql STABLE AS $path$
DECLARE source_id uuid; artifact uuid; representation uuid; actual_artifact uuid; actual_origin text; row record; sources integer:=0; locator jsonb;
BEGIN
 IF source_kind='evidence_locator' THEN
  locator:=source_value::jsonb;
  RETURN material_source_read_allowed(owner_subject,'occurrence',locator->>'occurrence',origins,explicit_refs) AND material_source_read_allowed(owner_subject,locator->>'kind',locator->>'id',origins,explicit_refs,(locator->>'occurrence')::uuid);
 END IF;
 source_id:=source_value::uuid;
 IF explicit_refs @> jsonb_build_array(jsonb_build_object('kind',source_kind,'id',source_value)) THEN
  RETURN EXISTS(SELECT 1 FROM material_lineage_read(owner_subject,ARRAY[source_kind],ARRAY[source_id],NULL) l WHERE l.present);
 END IF;
 CASE source_kind
 WHEN 'occurrence' THEN
  RETURN EXISTS(SELECT 1 FROM observation_occurrences o WHERE o.subject_id=owner_subject AND o.occurrence_id=source_id AND o.conversation_ref=ANY(origins) AND (o.artifact_id IS NOT NULL OR o.external_object_ref IS NOT NULL));
 WHEN 'artifact' THEN SELECT a.artifact_id INTO artifact FROM artifacts a WHERE a.subject_id=owner_subject AND a.artifact_id=source_id;
 WHEN 'source_region' THEN SELECT r.artifact_id INTO artifact FROM source_regions r WHERE r.subject_id=owner_subject AND r.source_region_id=source_id;
 WHEN 'derived_representation' THEN SELECT r.derived_representation_id INTO representation FROM derived_representations r WHERE r.subject_id=owner_subject AND r.derived_representation_id=source_id;
 WHEN 'derived_region' THEN SELECT r.derived_representation_id INTO representation FROM derived_regions r WHERE r.subject_id=owner_subject AND r.derived_region_id=source_id;
 ELSE RETURN false;
 END CASE;
 IF artifact IS NOT NULL THEN
  IF actual_occurrence IS NOT NULL THEN
   SELECT o.artifact_id,o.conversation_ref INTO actual_artifact,actual_origin FROM observation_occurrences o WHERE o.subject_id=owner_subject AND o.occurrence_id=actual_occurrence;
   IF actual_artifact=artifact AND material_source_read_allowed(owner_subject,'occurrence',actual_occurrence::text,origins,explicit_refs) THEN RETURN true; END IF;
  END IF;
  -- A bare locator has no attribution path. Only an unambiguous actual origin qualifies.
  RETURN (SELECT count(*)>0 AND count(DISTINCT o.conversation_ref)=1 AND bool_and(o.conversation_ref IS NOT NULL AND o.conversation_ref=ANY(origins)) FROM observation_occurrences o WHERE o.subject_id=owner_subject AND o.artifact_id=artifact);
 END IF;
 IF representation IS NOT NULL THEN
  FOR row IN SELECT r.source_region_id FROM representation_source_regions(owner_subject,representation) r LOOP
   sources:=sources+1;
   IF sources>1024 OR NOT material_source_read_allowed(owner_subject,'source_region',row.source_region_id::text,origins,explicit_refs,actual_occurrence) THEN RETURN false; END IF;
  END LOOP;
  RETURN sources>0;
 END IF;
 RETURN false;
EXCEPTION WHEN data_exception THEN RETURN false;
END $path$;

-- Exact WorkContext grants its task text only; anchors are separate candidates.
-- Recursion examines at most1024 metadata nodes/64 levels; unknown/cycles fail closed.
CREATE FUNCTION source_read_allowed(owner_subject uuid,source_kind text,source_value text,permitted_roots text[],work_contexts uuid[],permitted_origins text[] DEFAULT '{}',explicit_refs jsonb DEFAULT '[]',metadata_only boolean DEFAULT false)
RETURNS boolean LANGUAGE plpgsql STABLE AS $allowed$
DECLARE item record; total integer := 0; leaves integer := 0;
BEGIN
 IF source_kind='work_context' THEN
  RETURN EXISTS(SELECT 1 FROM work_contexts w WHERE w.subject_id=owner_subject AND w.work_context_id=source_value::uuid AND w.work_context_id=ANY(work_contexts));
 END IF;
 IF cardinality(permitted_roots)=0 AND cardinality(permitted_origins)=0 THEN RETURN false; END IF;
 FOR item IN
  WITH RECURSIVE walk(kind,value,path,cycle,metadata) AS (
   SELECT source_kind,source_value,ARRAY[source_kind||':'||source_value],false,metadata_only
   UNION ALL
   SELECT e.kind,e.value,w.path||(e.kind||':'||e.value),(e.kind||':'||e.value)=ANY(w.path),w.metadata OR e.metadata_edge
   FROM walk w CROSS JOIN LATERAL source_read_edges_with_mode(owner_subject,w.kind,w.value,w.metadata) e
   WHERE NOT w.cycle AND cardinality(w.path)<65
  ) SELECT w.*,NOT EXISTS(SELECT 1 FROM source_read_edges_with_mode(owner_subject,w.kind,w.value,w.metadata)) AS leaf FROM walk w LIMIT 1025
 LOOP
  total:=total+1;
  IF total>1024 OR item.cycle OR cardinality(item.path)>64 THEN RETURN false; END IF;
  IF item.leaf THEN
   leaves:=leaves+1;
   IF item.kind='work_context' THEN
    IF NOT EXISTS(SELECT 1 FROM work_contexts w WHERE w.subject_id=owner_subject AND w.work_context_id=item.value::uuid AND w.work_context_id=ANY(work_contexts)) THEN RETURN false; END IF;
    CONTINUE;
   END IF;
   IF item.kind NOT IN ('occurrence','artifact','source_region','derived_representation','derived_region','evidence_locator') THEN RETURN false; END IF;
   IF NOT material_source_read_allowed(owner_subject,item.kind,item.value,permitted_origins,explicit_refs) THEN RETURN false; END IF;
  END IF;
 END LOOP;
 RETURN total>0 AND leaves>0;
EXCEPTION WHEN invalid_text_representation OR data_exception THEN RETURN false;
END $allowed$;

-- Fixed metadata-only selector permission; never grants cognition body transmission.
CREATE FUNCTION source_read_metadata_allowed(uuid,text,text,text[],uuid[],text[],jsonb)
RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT source_read_allowed($1,$2,$3,$4,$5,$6,$7,true) $$;
