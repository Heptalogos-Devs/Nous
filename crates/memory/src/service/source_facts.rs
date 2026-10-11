//! Shared basis lineage SQL. Readbacks and bounded owner lanes use the same source facts.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use sqlx::{Postgres, QueryBuilder};

pub(super) fn push_source_facts(
    sql: &mut QueryBuilder<Postgres>,
    subject: SubjectId,
    roots: &[CognitiveRef],
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<()> {
    let (kinds, ids): (Vec<_>, Vec<_>) = roots
        .iter()
        .map(|reference| {
            let (kind, value) = reference_parts(reference);
            Ok((
                kind,
                value.parse::<Uuid>().map_err(|_| {
                    Error::Invalid("cognition source root must be an exact revision".into())
                })?,
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .unzip();
    sql.push("WITH RECURSIVE source_settings AS (SELECT ")
        .push_bind(subject.0)
        .push("::uuid subject, ")
        .push_bind(view.is_some())
        .push("::boolean historical, ")
        .push_bind(
            view.map(|view| view.schema_evidence_links.clone())
                .unwrap_or_default(),
        )
        .push("::uuid[] links, ")
        .push_bind(
            view.map(|view| view.entity_bindings.clone())
                .unwrap_or_default(),
        )
        .push("::uuid[] bindings, ")
        .push_bind(view.map(|view| view.as_of))
        .push("::timestamptz cut), source_roots AS (SELECT * FROM unnest(")
        .push_bind(kinds)
        .push("::text[], ")
        .push_bind(ids)
        .push("::uuid[]) r(root_kind,root)), ");
    sql.push(r#"
lineage(root_kind,root,kind,value) AS (
 SELECT root_kind,root,root_kind,root::text FROM source_roots
 UNION
 SELECT l.root_kind,l.root,e.kind,e.value FROM lineage l CROSS JOIN source_settings x CROSS JOIN LATERAL (
  SELECT m.ref_kind kind,m.ref_value value FROM episode_revision_members m WHERE m.episode_revision_id=CASE WHEN l.kind='episode_revision' THEN l.value::uuid END
  UNION SELECT CASE WHEN s.basis_kind='evidence' THEN 'occurrence' ELSE s.basis_kind END,CASE WHEN s.basis_kind='evidence' THEN s.occurrence_id::text ELSE s.basis_ref END FROM episode_revision_basis s WHERE s.episode_revision_id=CASE WHEN l.kind='episode_revision' THEN l.value::uuid END
  UNION SELECT 'occurrence',e.occurrence_id::text FROM memory_revision_evidence e WHERE e.memory_revision_id=CASE WHEN l.kind='memory_revision' THEN l.value::uuid END
  UNION SELECT d.target_ref_kind,d.target_ref FROM memory_revision_dependencies d WHERE d.memory_revision_id=CASE WHEN l.kind='memory_revision' THEN l.value::uuid END
  UNION SELECT s.ref_kind,s.ref_value FROM journal_revision_sources s WHERE s.journal_revision_id=CASE WHEN l.kind='journal_revision' THEN l.value::uuid END
  UNION SELECT CASE WHEN e.basis_kind='evidence' THEN 'occurrence' ELSE e.basis_kind END,CASE WHEN e.basis_kind='evidence' THEN e.occurrence_id::text ELSE e.basis_ref END FROM cognitive_schema_evidence_links e WHERE e.schema_revision_id=CASE WHEN l.kind='cognitive_schema_revision' THEN l.value::uuid END AND ((x.historical AND e.link_id=ANY(x.links)) OR (NOT x.historical AND e.revoked_at IS NULL))
 ) e WHERE e.value IS NOT NULL
), source_occurrences AS (
 SELECT DISTINCT l.root_kind,l.root,o.occurrence_id,o.source_class,o.actor_entity_ref,
  o.occurred_time_kind,o.occurred_time_start,o.occurred_time_end,o.observed_at,
  ARRAY(SELECT DISTINCT b.entity_ref FROM entity_mentions m CROSS JOIN LATERAL (
   SELECT entity_ref,binding_state FROM entity_binding_revisions WHERE mention_id=m.mention_id AND (NOT x.historical OR binding_revision_id=ANY(x.bindings)) ORDER BY revision_no DESC LIMIT 1
  ) b WHERE m.subject_id=x.subject AND m.occurrence_id=o.occurrence_id AND b.binding_state='bound' AND b.entity_ref IS NOT NULL) entities
 FROM lineage l CROSS JOIN source_settings x JOIN observation_occurrences o ON o.occurrence_id=CASE WHEN l.kind='occurrence' THEN l.value::uuid END
 WHERE o.subject_id=x.subject AND (NOT x.historical OR o.created_at<=x.cut)
)
"#);
    Ok(())
}
