//! Disposable projection inputs selected by semantic owners before candidate generation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_core::*;
use sqlx::Row;
use std::collections::{HashMap, HashSet};

pub struct HistoricalProjectionInput {
    pub sources: Vec<TextProjectionSource>,
    pub concepts: ConceptProjectionInput,
    pub topology: TopologyProjectionInput,
}

impl AuthorityStore {
    #[expect(
        clippy::too_many_lines,
        reason = "one repeatable-read projection assembles only owner-selected historical identities and their immutable content"
    )]
    pub async fn historical_projection_input(
        &self,
        view: &HistoricalAuthoritySnapshot,
        budget: EpisodeTextBudget,
    ) -> Result<HistoricalProjectionInput> {
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let memory_allowed: bool =
            sqlx::query_scalar("SELECT memory FROM subject_capabilities WHERE subject_id=$1")
                .bind(view.subject.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
        let (mut sources, mut tags) =
            super::text::historical_sources_in(&mut tx, view, budget, memory_allowed, None).await?;
        let indices: HashMap<_, _> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.reference.clone(), i))
            .collect();
        let mut nodes: HashSet<_> = sources.iter().map(|s| s.reference.clone()).collect();
        let mut edges = Vec::new();
        for source in &sources {
            for tag in &source.tag_ids {
                let target = CognitiveRef::Tag(TagId(
                    tag.parse()
                        .map_err(|_| Error::Infrastructure("invalid concept identity".into()))?,
                ));
                historical_pair(
                    &mut edges,
                    source.reference.clone(),
                    target,
                    "tag_attachment",
                    "derived_structure",
                    "positive",
                    ProjectionEdgeProvenance::Structure(format!(
                        "structure:tag:{}:{tag}",
                        source.reference
                    )),
                );
            }
            for entity in &source.entity_refs {
                let target = CognitiveRef::Entity(EntityRef::new(entity.clone())?);
                nodes.insert(target.clone());
                historical_pair(
                    &mut edges,
                    source.reference.clone(),
                    target,
                    "aboutness",
                    "derived_structure",
                    "positive",
                    ProjectionEdgeProvenance::Structure(format!(
                        "structure:aboutness:{}:{entity}",
                        source.reference
                    )),
                );
            }
        }
        let associations: Vec<_> = view
            .associations
            .iter()
            .filter(|_| memory_allowed)
            .map(|id| id.0)
            .collect();
        let rows = sqlx::query("SELECT * FROM association_evidence WHERE subject_id=$1 AND association_evidence_id=ANY($2::uuid[]) ORDER BY association_evidence_id")
            .bind(view.subject.0).bind(associations).fetch_all(&mut *tx).await.map_err(database_error)?;
        for row in rows {
            let from = historical_endpoint(
                view,
                &row.get::<String, _>("from_ref_kind"),
                &row.get::<String, _>("from_ref"),
            )?;
            let to = historical_endpoint(
                view,
                &row.get::<String, _>("to_ref_kind"),
                &row.get::<String, _>("to_ref"),
            )?;
            let (Some(from), Some(to)) = (from, to) else {
                continue;
            };
            if !nodes.contains(&from) || !nodes.contains(&to) || from == to {
                continue;
            }
            let relation: String = row.get("relation_kind");
            let polarity: String = row.get("polarity");
            if relation == "tag_attachment"
                && polarity == "positive"
                && let CognitiveRef::Tag(tag) = to
                && let Some(&index) = indices.get(&from)
            {
                sources[index].tag_ids.push(tag.0.to_string());
            }
            historical_pair(
                &mut edges,
                from,
                to,
                &relation,
                &row.get::<String, _>("basis_class"),
                &polarity,
                ProjectionEdgeProvenance::AuthorityBasis(CognitiveRef::Association(
                    AssociationEvidenceId(row.get("association_evidence_id")),
                )),
            );
        }
        let links: Vec<_> = if memory_allowed {
            view.schema_evidence_links.clone()
        } else {
            vec![]
        };
        let rows = sqlx::query("SELECT schema_revision_id,basis_kind,basis_ref FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND link_id=ANY($2::uuid[]) AND role='support' AND epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample') AND basis_kind<>'evidence'")
            .bind(view.subject.0).bind(links).fetch_all(&mut *tx).await.map_err(database_error)?;
        for row in rows {
            let from = CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(
                row.get("schema_revision_id"),
            ));
            let to = parse_reference(
                &row.get::<String, _>("basis_kind"),
                &row.get::<String, _>("basis_ref"),
            )?;
            if nodes.contains(&from) && nodes.contains(&to) {
                historical_pair(
                    &mut edges,
                    from.clone(),
                    to.clone(),
                    "schema_support",
                    "derived_structure",
                    "positive",
                    ProjectionEdgeProvenance::Structure(format!("structure:schema:{from}:{to}")),
                );
            }
        }
        let (selected_kinds, selected_values): (Vec<_>, Vec<_>) = sources
            .iter()
            .map(|s| reference_parts(&s.reference))
            .unzip();
        let rows=sqlx::query(r#"
WITH selected AS (SELECT * FROM unnest($2::text[],$3::text[]) r(kind,value)), links AS (
 SELECT 'memory_revision' from_kind,d.memory_revision_id::text from_value,d.target_ref_kind to_kind,d.target_ref to_value FROM memory_revision_dependencies d JOIN selected s ON s.kind='memory_revision' AND s.value=d.memory_revision_id::text WHERE d.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample')
 UNION ALL SELECT 'journal_revision',d.journal_revision_id::text,d.ref_kind,d.ref_value FROM journal_revision_sources d JOIN selected s ON s.kind='journal_revision' AND s.value=d.journal_revision_id::text
 UNION ALL SELECT 'episode_revision',d.episode_revision_id::text,d.ref_kind,d.ref_value FROM episode_revision_members d JOIN selected s ON s.kind='episode_revision' AND s.value=d.episode_revision_id::text
 UNION ALL SELECT 'episode_revision',d.episode_revision_id::text,d.basis_kind,d.basis_ref FROM episode_revision_basis d JOIN selected s ON s.kind='episode_revision' AND s.value=d.episode_revision_id::text WHERE d.basis_kind<>'evidence' AND d.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample')
) SELECT * FROM links WHERE $1::uuid IS NOT NULL ORDER BY from_kind,from_value,to_kind,to_value
"#).bind(view.subject.0).bind(selected_kinds).bind(selected_values).fetch_all(&mut *tx).await.map_err(database_error)?;
        for row in rows {
            let from = parse_reference(
                &row.get::<String, _>("from_kind"),
                &row.get::<String, _>("from_value"),
            )?;
            let to = parse_reference(
                &row.get::<String, _>("to_kind"),
                &row.get::<String, _>("to_value"),
            )?;
            if nodes.contains(&from) && nodes.contains(&to) && from != to {
                historical_pair(
                    &mut edges,
                    from.clone(),
                    to.clone(),
                    "cognition_basis",
                    "derived_structure",
                    "positive",
                    ProjectionEdgeProvenance::Structure(format!(
                        "structure:cognition-basis:{from}:{to}"
                    )),
                );
            }
        }
        for source in &mut sources {
            source.tag_ids.sort();
            source.tag_ids.dedup();
            for tag in &source.tag_ids {
                if let Some(concept) = tags.iter_mut().find(|t| t.tag.0.to_string() == *tag) {
                    concept.attachments.push(source.reference.clone());
                }
            }
        }
        for tag in &mut tags {
            tag.attachments.sort_by_key(ToString::to_string);
            tag.attachments.dedup();
        }
        sources.sort_by_key(|source| source.reference.to_string());
        tx.commit().await.map_err(database_error)?;
        Ok(HistoricalProjectionInput {
            sources,
            concepts: ConceptProjectionInput { watermark: 0, tags },
            topology: TopologyProjectionInput {
                watermark: 0,
                nodes: {
                    let mut nodes: Vec<_> = nodes.into_iter().collect();
                    nodes.sort_by_key(ToString::to_string);
                    nodes
                },
                edges,
            },
        })
    }
}
fn historical_endpoint(
    view: &HistoricalAuthoritySnapshot,
    kind: &str,
    value: &str,
) -> Result<Option<CognitiveRef>> {
    let reference = parse_reference(kind, value)?;
    Ok(match reference {
        CognitiveRef::Tag(tag) => view.canonical_tag(tag).map(CognitiveRef::Tag),
        _ => Some(
            view.cognition_for(&reference)
                .filter(|state| state.object == reference)
                .map_or(reference.clone(), |state| state.head.clone()),
        ),
    })
}
fn historical_pair(
    edges: &mut Vec<TopologyEdgeSource>,
    from: CognitiveRef,
    to: CognitiveRef,
    kind: &str,
    class: &str,
    polarity: &str,
    provenance: ProjectionEdgeProvenance,
) {
    let symmetric = match &provenance {
        ProjectionEdgeProvenance::Structure(_) => true,
        ProjectionEdgeProvenance::AuthorityBasis(_) => {
            serde_json::from_value::<TopologyRelation>(serde_json::Value::String(kind.into()))
                .is_ok_and(|relation| relation.is_symmetric())
        }
    };
    for (index, (from, to)) in [(from.clone(), to.clone()), (to, from)]
        .into_iter()
        .enumerate()
    {
        if index > 0 && !symmetric {
            continue;
        }
        edges.push(TopologyEdgeSource {
            from,
            to,
            basis_class: class.into(),
            association_kind: kind.into(),
            polarity: polarity.into(),
            support_mass: 1.0,
            provenance: provenance.clone(),
        });
    }
}
