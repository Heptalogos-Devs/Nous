//! Current cognitive nodes and exact source basis, projected as structure.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::projection::topology::TopologyEdgeSource;
use crate::{database_error as db, *};
use nous_core::*;
use sqlx::Row;
use std::collections::HashSet;

pub(crate) async fn append(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    nodes: &mut HashSet<CognitiveRef>,
    edges: &mut Vec<TopologyEdgeSource>,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<()> {
    let live = selected_cognition_in(tx, subject, view).await?;
    nodes.extend(live.iter().cloned());
    let rows = sqlx::query(r#"
SELECT 'memory_revision' from_kind,d.memory_revision_id::text from_value,d.target_ref_kind to_kind,d.target_ref to_value FROM memory_revision_dependencies d JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=$1 AND d.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample')
UNION ALL SELECT 'journal_revision',s.journal_revision_id::text,s.ref_kind,s.ref_value FROM journal_revision_sources s JOIN journal_revisions r USING(journal_revision_id) WHERE r.subject_id=$1
UNION ALL SELECT 'episode_revision',m.episode_revision_id::text,m.ref_kind,m.ref_value FROM episode_revision_members m JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=$1
UNION ALL SELECT 'episode_revision',s.episode_revision_id::text,s.basis_kind,s.basis_ref FROM episode_revision_basis s JOIN episode_revisions r USING(episode_revision_id) WHERE r.subject_id=$1 AND s.basis_kind<>'evidence' AND s.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample')
UNION ALL SELECT 'cognitive_schema_revision',s.schema_revision_id::text,s.basis_kind,s.basis_ref FROM cognitive_schema_evidence_links s WHERE s.subject_id=$1 AND s.basis_kind<>'evidence' AND (($2::uuid[] IS NULL AND s.revoked_at IS NULL) OR s.link_id=ANY($2)) AND s.role='support' AND s.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample')
"#).bind(subject.0).bind(view.map(|view| &view.schema_evidence_links)).fetch_all(&mut **tx).await.map_err(db)?;
    let mut pairs = HashSet::new();
    for row in rows {
        let from = parse_reference(
            &row.get::<String, _>("from_kind"),
            &row.get::<String, _>("from_value"),
        )?;
        let kind: String = row.get("to_kind");
        if !matches!(
            kind.as_str(),
            "memory_revision"
                | "episode_revision"
                | "journal_revision"
                | "cognitive_schema_revision"
        ) {
            continue;
        }
        let to = parse_reference(&kind, &row.get::<String, _>("to_value"))?;
        if from == to
            || !live.contains(&from)
            || !live.contains(&to)
            || !pairs.insert((from.clone(), to.clone()))
        {
            continue;
        }
        // Traversal in both directions has the same exact basis identity;
        // adjacency makes no assertion of causal direction or independence.
        let root = format!("structure:cognition-basis:{from}:{to}");
        for (source, target) in [(from.clone(), to.clone()), (to, from)] {
            edges.push(TopologyEdgeSource {
                from: source,
                to: target,
                basis_class: "derived_structure".into(),
                association_kind: "cognition_basis".into(),
                polarity: "positive".into(),
                support_mass: 1.0,
                provenance: ProjectionEdgeProvenance::Structure(root.clone()),
            });
        }
    }
    Ok(())
}

/// Owner-selected revisions, with current purge represented by the owning rows.
pub(crate) async fn selected_cognition_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<HashSet<CognitiveRef>> {
    let selected = view.map(|view| {
        view.cognition
            .iter()
            .filter(|state| {
                state.state["acceptance_state"] == "accepted"
                    && state.state["integrity_state"] == "valid"
                    && state.state["suppression_state"] == "normal"
            })
            .flat_map(|state| state.revisions.iter())
            .map(reference_parts)
            .collect::<Vec<_>>()
    });
    let kinds = selected.as_ref().map(|rows| {
        rows.iter()
            .map(|(kind, _)| kind.clone())
            .collect::<Vec<_>>()
    });
    let values = selected.as_ref().map(|rows| {
        rows.iter()
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>()
    });
    let rows = sqlx::query(r#"
WITH known AS (
 SELECT 'memory_revision' kind,r.memory_revision_id::text value,o.current_revision_id=r.memory_revision_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' eligible FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE o.subject_id=$1
 UNION ALL SELECT 'episode_revision',r.episode_revision_id::text,o.current_revision_id=r.episode_revision_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE o.subject_id=$1
 UNION ALL SELECT 'journal_revision',r.journal_revision_id::text,o.current_revision_id=r.journal_revision_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE o.subject_id=$1
 UNION ALL SELECT 'cognitive_schema_revision',r.schema_revision_id::text,o.current_revision_id=r.schema_revision_id AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1
) SELECT kind,value FROM known WHERE ($2::text[] IS NULL AND eligible) OR (kind,value) IN (SELECT * FROM unnest($2::text[],$3::text[]))
"#).bind(subject.0).bind(kinds).bind(values).fetch_all(&mut **tx).await.map_err(db)?;
    rows.into_iter()
        .map(|row| {
            parse_reference(
                &row.get::<String, _>("kind"),
                &row.get::<String, _>("value"),
            )
        })
        .collect()
}
