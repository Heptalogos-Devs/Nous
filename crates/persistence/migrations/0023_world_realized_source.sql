-- Canonical virtual event provenance remains distinct from arbitrary context.
ALTER TABLE observation_occurrences ADD COLUMN world_realized jsonb;
ALTER TABLE observation_occurrences ADD CONSTRAINT canonical_world_source
CHECK ((source_class = 'world_realized') = (world_realized IS NOT NULL));
CREATE INDEX observation_world_event ON observation_occurrences
(subject_id, (world_realized->>'world_id'), (world_realized->>'event_id'))
WHERE world_realized IS NOT NULL;
-- Actual observed_at is immutable. Episode chronology has its own explicit domain.
ALTER TABLE experience_items ADD COLUMN experience_at timestamptz,
 ADD COLUMN experience_time_domain_ref text NOT NULL DEFAULT 'time:real';
UPDATE experience_items SET experience_at=observed_at;
ALTER TABLE experience_items ALTER COLUMN experience_at SET NOT NULL;
ALTER TABLE episode_drafts ADD COLUMN experience_time_domain_ref text NOT NULL DEFAULT 'time:real';
CREATE INDEX experience_story_chronology ON experience_items(subject_id,experience_time_domain_ref,experience_at,recorded_seq);

CREATE OR REPLACE FUNCTION material_lineage_read(uuid,text[],uuid[],timestamptz)
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
 SELECT s.artifact_id AS origin,o.occurrence_id,o.artifact_id,o.source_class,o.external_object_ref,o.world_realized FROM (SELECT DISTINCT artifact_id FROM sources) s
 JOIN observation_occurrences o ON o.subject_id=$1 AND o.artifact_id=s.artifact_id AND ($4::timestamptz IS NULL OR o.created_at<=$4)
 UNION SELECT p.origin,o.occurrence_id,o.artifact_id,o.source_class,o.external_object_ref,o.world_realized FROM lineage p
 JOIN observation_occurrences o ON (o.artifact_id IS NOT NULL AND o.artifact_id=p.artifact_id) OR (o.external_object_ref IS NOT NULL AND o.external_object_ref=p.external_object_ref AND o.source_class=p.source_class)
 WHERE o.subject_id=$1 AND ($4::timestamptz IS NULL OR o.created_at<=$4)
), external_roots AS (
 SELECT origin,MIN(CASE WHEN world_realized IS NOT NULL THEN 'world:'||(world_realized->>'world_id')||':'||COALESCE(world_realized->'shared_origin_refs'->>0,world_realized->>'event_id') ELSE 'external:'||source_class||':'||external_object_ref END) AS root FROM lineage WHERE external_object_ref IS NOT NULL OR world_realized IS NOT NULL GROUP BY origin
)
SELECT r.ordinality,r.present,r.derived_id,r.source_class,r.external_object_ref,
 ARRAY(SELECT DISTINCT s.artifact_id FROM sources s WHERE s.ordinality=r.ordinality) AS artifacts,
 ARRAY(SELECT DISTINCT coalesce(e.root,'artifact:'||s.artifact_id) FROM sources s LEFT JOIN external_roots e ON e.origin=s.artifact_id WHERE s.ordinality=r.ordinality) AS roots
FROM resolved r

$lineage$;

-- Discovery structure participates in the same semantic knowledge cut.
ALTER TABLE association_evidence ADD COLUMN formed_stamp jsonb;
UPDATE association_evidence SET formed_stamp=jsonb_build_object('time_domain_ref','time:real','at',created_at,'recorded_at',created_at);
ALTER TABLE association_evidence ALTER COLUMN formed_stamp SET NOT NULL,
 ALTER COLUMN formed_stamp SET DEFAULT COALESCE(NULLIF(current_setting('nous.formed_stamp',true),'')::jsonb,authority_semantic_stamp());
ALTER TABLE cognitive_schema_evidence_links ADD COLUMN formed_stamp jsonb;
UPDATE cognitive_schema_evidence_links SET formed_stamp=jsonb_build_object('time_domain_ref','time:real','at',created_at,'recorded_at',created_at);
ALTER TABLE cognitive_schema_evidence_links ALTER COLUMN formed_stamp SET NOT NULL,
 ALTER COLUMN formed_stamp SET DEFAULT COALESCE(NULLIF(current_setting('nous.formed_stamp',true),'')::jsonb,authority_semantic_stamp());
