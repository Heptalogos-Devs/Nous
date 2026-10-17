-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- Material admission reuses Runtime's actual-use ledger; it grants no cognition
-- feedback authority. Exact paired provenance is validated by the owner.
ALTER TABLE cognitive_use_events DROP CONSTRAINT cognitive_use_events_ref_kind_check;
ALTER TABLE cognitive_use_events ADD CONSTRAINT cognitive_use_events_ref_kind_check CHECK (
 ref_kind IN ('memory_revision','cognitive_schema_revision','episode_revision','journal_revision','self_facet_revision','narrative_thread_revision','social_relation_revision','language_convention_revision','desire_revision','concern_revision','desired_condition_revision')
 OR (ref_kind IN ('occurrence','source_region','derived_representation','derived_region') AND use_kind='presented' AND session_id IS NOT NULL AND context @> '{"model_input":true}'::jsonb)
);
