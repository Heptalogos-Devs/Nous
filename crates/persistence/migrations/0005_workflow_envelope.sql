-- One-time transition of saved operations. Domain JSON is preserved verbatim;
-- only former shared metadata moves into the explicit envelope.
ALTER TABLE model_workflow_operations ADD COLUMN dependencies jsonb NOT NULL DEFAULT '[]';
-- A parent workflow may commit several independently atomic child mutations.
ALTER TABLE model_workflow_operations ADD COLUMN mutation_operations uuid[] NOT NULL DEFAULT '{}';

-- Old split receipts stored only the parent id. Recover the single recorded
-- child batch; do not silently attribute multiple historical splits to one call.
DO $$ BEGIN
    IF EXISTS (
        SELECT 1 FROM mutation_receipts receipt
        WHERE receipt.result_kind='schema_split' AND receipt.result_ref !~ '^\['
        GROUP BY receipt.subject_id,receipt.result_ref
        HAVING count(*)>1
    ) THEN RAISE EXCEPTION 'Ambiguous legacy schema split receipt: supply its exact child revision batch before migration';
    END IF;
END $$;
UPDATE mutation_receipts receipt SET result_ref=(
    SELECT COALESCE(jsonb_agg(lineage.from_revision_id ORDER BY lineage.from_revision_id),'[]')::text
    FROM cognitive_schema_revisions parent
    JOIN cognitive_schema_lineage lineage ON lineage.to_revision_id=parent.schema_revision_id
        AND lineage.relation='schema_split_from'
    WHERE parent.schema_id::text=receipt.result_ref
) WHERE receipt.result_kind='schema_split' AND receipt.result_ref !~ '^\[';

CREATE FUNCTION pg_temp.workflow_dependencies(payload jsonb, owner_name text)
RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE
    result jsonb := '[]';
    field_name text;
    member jsonb;
    ref_kind text;
    ref_id text;
BEGIN
    IF jsonb_typeof(payload) = 'array' THEN
        FOR member IN SELECT value FROM jsonb_array_elements(payload) LOOP
            result := result || pg_temp.workflow_dependencies(member, owner_name);
        END LOOP;
    ELSIF jsonb_typeof(payload) = 'object' THEN
        IF payload->>'kind' IN ('memory','memory_revision','episode','episode_revision',
            'journal','journal_revision','cognitive_schema','cognitive_schema_revision','cognitive_seed_version',
            'artifact','occurrence','source_region','derived_representation','derived_region',
            'tag','association','resource','entity','session','subject','work_context')
            AND jsonb_typeof(payload->'value') = 'string' THEN
            result := result || jsonb_build_array(jsonb_build_object('kind',payload->>'kind','id',payload->>'value'));
        END IF;
        FOR field_name, member IN SELECT key,value FROM jsonb_each(payload) LOOP
            -- These owner fields contain opaque user/provider data, not protocol references.
            IF field_name IN ('context','metadata','provenance','quality','structuredPayload','configuration','model') THEN
                CONTINUE;
            END IF;
            IF jsonb_typeof(member) IN ('object','array') THEN
                result := result || pg_temp.workflow_dependencies(member, owner_name);
            ELSIF jsonb_typeof(member) = 'string' THEN
                ref_kind := CASE field_name
                    WHEN 'memoryId' THEN 'memory' WHEN 'memoryRevisionId' THEN 'memory_revision'
                    WHEN 'episodeId' THEN 'episode' WHEN 'episodeRevisionId' THEN 'episode_revision'
                    WHEN 'journalId' THEN 'journal' WHEN 'journalRevisionId' THEN 'journal_revision'
                    WHEN 'schemaId' THEN 'cognitive_schema' WHEN 'schemaRevisionId' THEN 'cognitive_schema_revision'
                    WHEN 'artifactId' THEN 'artifact' WHEN 'occurrenceId' THEN 'occurrence'
                    WHEN 'groundingOccurrenceId' THEN 'occurrence' WHEN 'sourceRegionId' THEN 'source_region'
                    WHEN 'representationId' THEN 'derived_representation'
                    WHEN 'derivedRepresentationId' THEN 'derived_representation' WHEN 'derivedRegionId' THEN 'derived_region'
                    WHEN 'tagId' THEN 'tag' WHEN 'associationId' THEN 'association'
                    WHEN 'revisionId' THEN CASE
                        WHEN payload ? 'schemaId' THEN 'cognitive_schema_revision'
                        WHEN payload ? 'episodeId' THEN 'episode_revision'
                        WHEN payload ? 'journalId' THEN 'journal_revision'
                        WHEN owner_name = 'memory' THEN 'memory_revision' END
                    ELSE NULL END;
                ref_id := member #>> '{}';
                IF ref_kind IS NOT NULL AND ref_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' THEN
                    result := result || jsonb_build_array(jsonb_build_object('kind',ref_kind,'id',lower(ref_id)));
                END IF;
            END IF;
        END LOOP;
    END IF;
    RETURN result;
END;
$$;

UPDATE model_workflow_operations w SET dependencies = (
    SELECT COALESCE(jsonb_agg(DISTINCT ref),'[]') FROM jsonb_array_elements(
        pg_temp.workflow_dependencies(w.snapshot,w.owner)
        || pg_temp.workflow_dependencies(w.proposal,w.owner)
        || pg_temp.workflow_dependencies(w.outcome,w.owner)
        || COALESCE((SELECT jsonb_build_array(jsonb_build_object('kind',CASE r.result_kind
            WHEN 'memory' THEN 'memory_revision' WHEN 'schema' THEN 'cognitive_schema_revision'
            WHEN 'episode' THEN 'episode_revision' WHEN 'journal' THEN 'journal_revision' END,
            'id',r.result_revision::text)) FROM mutation_receipts r
            WHERE r.subject_id=w.subject_id AND r.operation_id::text=w.operation_key
            AND r.result_revision IS NOT NULL AND r.result_kind IN ('memory','schema','episode','journal')),'[]')
    ) ref
);

UPDATE model_workflow_operations SET
    snapshot = jsonb_build_object(
        'content',jsonb_build_object('payload',snapshot - 'cognitive_formed_at' - 'maintenance_claim','dependencies',dependencies,'purged',false),
        'cognitive_formed_at',snapshot->'cognitive_formed_at',
        'maintenance_claim',CASE WHEN snapshot ? 'maintenance_claim' THEN jsonb_build_object(
            'need_id',snapshot->'maintenance_claim'->'need_id',
            'lease_token',snapshot->'maintenance_claim'->'lease_token',
            'trigger_authority_seq',COALESCE(snapshot->'maintenance_claim'->'trigger','0'::jsonb),
            'trigger_revision',snapshot->'maintenance_claim'->'trigger_revision') ELSE NULL END),
    proposal = CASE WHEN proposal IS NULL THEN NULL ELSE jsonb_build_object(
        'payload',proposal,'dependencies',pg_temp.workflow_dependencies(proposal,owner),'purged',false) END,
    outcome = CASE WHEN outcome IS NULL THEN NULL ELSE jsonb_build_object(
        'payload',CASE WHEN outcome = '{"purged":true}' THEN 'null'::jsonb ELSE outcome END,
        'dependencies',pg_temp.workflow_dependencies(outcome,owner),'purged',outcome = '{"purged":true}') END;

DROP FUNCTION pg_temp.workflow_dependencies(jsonb,text);
