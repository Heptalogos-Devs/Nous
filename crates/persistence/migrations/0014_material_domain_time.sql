-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- Physical contact and authority recording remain UTC. A semantic observation is
-- captured independently by the Subject clock in the committing transaction.
ALTER TABLE observation_occurrences
 ADD COLUMN occurred_time_domain_ref text NOT NULL DEFAULT 'time:real'
   CHECK(length(occurred_time_domain_ref) BETWEEN 1 AND 256),
 ADD COLUMN observed_stamp jsonb;
UPDATE observation_occurrences SET observed_stamp=jsonb_build_object(
 'time_domain_ref','time:real','at',observed_at,'recorded_at',created_at);
ALTER TABLE observation_occurrences ALTER COLUMN observed_stamp SET NOT NULL;
-- Stored request receipts are owner records, not paid provider payloads. Preserve
-- their request digests; the real-domain default does not change request identity.
UPDATE observation_request_bindings b SET accepted=jsonb_set(b.accepted,'{occurrence}',
 (b.accepted->'occurrence') || jsonb_build_object(
   'occurred_time_domain_ref',o.occurred_time_domain_ref,'observed_stamp',o.observed_stamp))
FROM observation_occurrences o WHERE o.occurrence_id=b.occurrence_id;

-- This original bare-extent interface is explicitly real-domain. Owners which do
-- not yet consume domain metadata must see UNKNOWN, never a numeric world time.
CREATE OR REPLACE FUNCTION material_occurrence_facts(owner_subject uuid, cutoff timestamptz, binding_ids uuid[])
RETURNS TABLE (occurrence_id uuid, source_class text, actor_entity_ref text,
               occurred_time_kind text, occurred_time_start timestamptz, occurred_time_end timestamptz,
               observed_at timestamptz, entities text[])
LANGUAGE SQL STABLE AS $$
    SELECT o.occurrence_id,o.source_class,o.actor_entity_ref,
           CASE WHEN o.occurred_time_domain_ref='time:real' THEN o.occurred_time_kind ELSE 'unknown' END,
           CASE WHEN o.occurred_time_domain_ref='time:real' THEN o.occurred_time_start END,
           CASE WHEN o.occurred_time_domain_ref='time:real' THEN o.occurred_time_end END,o.observed_at,
           ARRAY(SELECT DISTINCT b.entity_ref FROM entity_mentions m
                 CROSS JOIN LATERAL (SELECT entity_ref,binding_state FROM entity_binding_revisions
                                     WHERE mention_id=m.mention_id AND (binding_ids IS NULL OR binding_revision_id=ANY(binding_ids))
                                     ORDER BY revision_no DESC LIMIT 1) b
                 WHERE m.subject_id=owner_subject AND m.occurrence_id=o.occurrence_id
                   AND b.binding_state='bound' AND b.entity_ref IS NOT NULL)
    FROM observation_occurrences o
    WHERE o.subject_id=owner_subject AND (cutoff IS NULL OR o.created_at<=cutoff)
$$;

ALTER TABLE derived_representations ADD COLUMN formed_stamp jsonb;
UPDATE derived_representations SET formed_stamp=jsonb_build_object(
 'time_domain_ref','time:real','at',created_at,'recorded_at',created_at);
ALTER TABLE derived_representations ALTER COLUMN formed_stamp SET NOT NULL;
