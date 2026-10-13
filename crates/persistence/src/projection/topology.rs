// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use nous_core::*;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyEdgeSource {
    pub from: CognitiveRef,
    pub to: CognitiveRef,
    pub basis_class: String,
    pub association_kind: String,
    pub polarity: String,
    pub support_mass: f64,
    pub provenance: ProjectionEdgeProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectionEdgeProvenance {
    Structure(String),
    AuthorityBasis(CognitiveRef),
}

pub struct VcpProjectionRows {
    pub authority_watermark: i64,
    pub watermarks: super::ProjectionWatermarks,
    pub topology: TopologyProjectionInput,
    pub sources: Vec<crate::TextProjectionSource>,
}

pub struct TopologyProjectionInput {
    pub watermark: i64,
    pub nodes: Vec<CognitiveRef>,
    pub edges: Vec<TopologyEdgeSource>,
}

fn link(
    from: CognitiveRef,
    to: CognitiveRef,
    kind: &str,
    structure_identity: String,
) -> TopologyEdgeSource {
    TopologyEdgeSource {
        from,
        to,
        basis_class: "derived_structure".into(),
        association_kind: kind.into(),
        polarity: "positive".into(),
        support_mass: 1.0,
        provenance: ProjectionEdgeProvenance::Structure(structure_identity),
    }
}

impl AuthorityStore {
    pub async fn topology_projection_input(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<TopologyProjectionInput> {
        let mut tx = self.read_view().await?;
        if view.is_some_and(|view| view.subject != subject) {
            return Err(Error::Invalid("topology view Subject mismatch".into()));
        }
        let memory_enabled = memory_enabled
            && sqlx::query_scalar::<_, bool>(
                "SELECT memory FROM subject_capabilities WHERE subject_id=$1",
            )
            .bind(subject.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        let watermark = if view.is_some() {
            0
        } else {
            super::watermark(&mut tx, subject, "topology", "").await?
        };
        let result =
            topology_snapshot_in(&mut tx, subject, memory_enabled, watermark, view).await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    /// Topology, text representations and curve memberships from one Authority
    /// snapshot. Embeddings are resolved by the serving adapter after this read.
    pub async fn vcp_projection_rows(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        budget: crate::EpisodeTextBudget,
        space: &str,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<VcpProjectionRows> {
        let mut tx = self.read_view().await?;
        let authority_watermark: i64 =
            sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1")
                .bind(subject.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        let watermarks = if view.is_some() {
            Default::default()
        } else {
            super::watermarks_in(&mut tx, subject).await?
        };
        let topology = topology_snapshot_in(
            &mut tx,
            subject,
            memory_enabled,
            watermarks.desired("topology", space),
            view,
        )
        .await?;
        let mut sources =
            super::text::sources_in(&mut tx, subject, memory_enabled, budget, view).await?;
        sources.sort_by_key(|s| s.reference.to_string());
        tx.commit().await.map_err(db)?;
        Ok(VcpProjectionRows {
            authority_watermark,
            watermarks,
            topology,
            sources,
        })
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "topology projection assembles one repeatable-read Authority snapshot"
)]
async fn topology_snapshot_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    memory_enabled: bool,
    watermark: i64,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<TopologyProjectionInput> {
    let mut nodes = HashSet::new();
    let mut edges = Vec::new();
    if memory_enabled {
        crate::projection::longitudinal::append(tx, subject, &mut nodes, &mut edges, view).await?;
        let memory_ids: Vec<_> = nodes
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::MemoryRevision(id) => Some(id.0),
                _ => None,
            })
            .collect();
        let schema_ids: Vec<_> = nodes
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::CognitiveSchemaRevision(id) => Some(id.0),
                _ => None,
            })
            .collect();
        let rows = sqlx::query(r#"
SELECT 'tag' kind,t.memory_revision_id from_revision,t.tag_id::text target FROM memory_revision_tags t WHERE t.memory_revision_id=ANY($1::uuid[])
UNION ALL SELECT 'entity',m.memory_revision_id,m.entity_ref FROM memory_revision_aboutness m WHERE m.memory_revision_id=ANY($1::uuid[])
UNION ALL SELECT 'schema',l.basis_ref::uuid,l.schema_revision_id::text FROM cognitive_schema_evidence_links l WHERE l.schema_revision_id=ANY($2::uuid[]) AND l.basis_kind='memory_revision' AND l.basis_ref=ANY($3::text[]) AND l.role='support' AND l.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample') AND (($4::uuid[] IS NULL AND l.revoked_at IS NULL) OR l.link_id=ANY($4))
"#).bind(&memory_ids).bind(schema_ids).bind(memory_ids.iter().map(ToString::to_string).collect::<Vec<_>>()).bind(view.map(|view| &view.schema_evidence_links)).fetch_all(&mut **tx).await.map_err(db)?;
        for row in rows {
            let from = CognitiveRef::MemoryRevision(MemoryRevisionId(
                row.try_get("from_revision").map_err(db)?,
            ));
            let kind: String = row.try_get("kind").map_err(db)?;
            let value: String = row.try_get("target").map_err(db)?;
            let (to, relation, identity) = match kind.as_str() {
                "tag" => {
                    let raw = CognitiveRef::Tag(TagId(
                        value
                            .parse()
                            .map_err(|_| Error::Infrastructure("invalid tag identity".into()))?,
                    ));
                    let Some(to) = canonical_endpoint_in(tx, subject, raw, view).await? else {
                        continue;
                    };
                    (to, "tag_attachment", "tag")
                }
                "entity" => (
                    CognitiveRef::Entity(EntityRef::new(value)?),
                    "aboutness",
                    "aboutness",
                ),
                "schema" => (
                    CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(
                        value
                            .parse()
                            .map_err(|_| Error::Infrastructure("invalid schema identity".into()))?,
                    )),
                    "schema_support",
                    "schema",
                ),
                _ => unreachable!(),
            };
            nodes.insert(to.clone());
            let root = format!("structure:{identity}:{from}:{to}");
            edges.push(link(from.clone(), to.clone(), relation, root.clone()));
            edges.push(link(to, from, relation, root));
        }

        let association_rows = sqlx::query(
                "SELECT association_evidence_id,from_ref_kind,from_ref,to_ref_kind,to_ref,basis_class,relation_kind,polarity FROM association_evidence WHERE subject_id=$1 AND (($2::uuid[] IS NULL AND revoked_at IS NULL) OR association_evidence_id=ANY($2))",
            )
            .bind(subject.0)
            .bind(view.map(|view| view.associations.iter().map(|id| id.0).collect::<Vec<_>>()))
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?;
        for row in association_rows {
            let association_id: Uuid = row.try_get("association_evidence_id").map_err(db)?;
            let from = parse_reference(
                &row.try_get::<String, _>("from_ref_kind").map_err(db)?,
                &row.try_get::<String, _>("from_ref").map_err(db)?,
            )?;
            let to = parse_reference(
                &row.try_get::<String, _>("to_ref_kind").map_err(db)?,
                &row.try_get::<String, _>("to_ref").map_err(db)?,
            )?;
            let Some(from) = canonical_endpoint_in(tx, subject, from, view).await? else {
                continue;
            };
            let Some(to) = canonical_endpoint_in(tx, subject, to, view).await? else {
                continue;
            };
            if from == to {
                continue;
            }
            if !allowed(&from) || !allowed(&to) {
                continue;
            }
            let current_cognition = |reference: &CognitiveRef| {
                !matches!(
                    reference,
                    CognitiveRef::MemoryRevision(_)
                        | CognitiveRef::EpisodeRevision(_)
                        | CognitiveRef::JournalRevision(_)
                        | CognitiveRef::CognitiveSchemaRevision(_)
                ) || nodes.contains(reference)
            };
            if !current_cognition(&from) || !current_cognition(&to) {
                continue;
            }
            nodes.insert(from.clone());
            nodes.insert(to.clone());
            let basis_class: String = row.try_get("basis_class").map_err(db)?;
            let relation_kind: String = row.try_get("relation_kind").map_err(db)?;
            let polarity: String = row.try_get("polarity").map_err(db)?;
            let provenance = ProjectionEdgeProvenance::AuthorityBasis(CognitiveRef::Association(
                AssociationEvidenceId(association_id),
            ));
            edges.push(TopologyEdgeSource {
                from: from.clone(),
                to: to.clone(),
                basis_class: basis_class.clone(),
                association_kind: relation_kind.clone(),
                polarity: polarity.clone(),
                support_mass: 1.0,
                provenance: provenance.clone(),
            });
            let symmetric = serde_json::from_value::<TopologyRelation>(serde_json::Value::String(
                relation_kind.clone(),
            ))
            .is_ok_and(|kind| kind.is_symmetric());
            if symmetric {
                edges.push(TopologyEdgeSource {
                    from: to,
                    to: from,
                    basis_class,
                    association_kind: relation_kind,
                    polarity,
                    support_mass: 1.0,
                    provenance,
                });
            }
        }

        let rows = sqlx::query(
                "SELECT rr.from_revision_id,rr.to_revision_id,rr.relation FROM memory_revision_relations rr JOIN memory_revisions l ON l.memory_revision_id=rr.from_revision_id JOIN memory_revisions r ON r.memory_revision_id=rr.to_revision_id JOIN memory_objects lo ON lo.memory_id=l.memory_id JOIN memory_objects ro ON ro.memory_id=r.memory_id WHERE lo.subject_id=$1 AND ro.subject_id=$1 AND l.memory_revision_id=ANY($2::uuid[]) AND r.memory_revision_id=ANY($2)",
            )
            .bind(subject.0)
            .bind(&memory_ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?;
        for row in rows {
            let from = CognitiveRef::MemoryRevision(MemoryRevisionId(
                row.try_get("from_revision_id").map_err(db)?,
            ));
            let to = CognitiveRef::MemoryRevision(MemoryRevisionId(
                row.try_get("to_revision_id").map_err(db)?,
            ));
            nodes.insert(from.clone());
            nodes.insert(to.clone());
            let relation: String = row.try_get("relation").map_err(db)?;
            let root = format!("structure:relation:{from}:{to}:{relation}");
            edges.push(link(from, to, &relation, root));
        }
    }
    for resource in
        sqlx::query_scalar::<_, String>("SELECT resource_ref FROM resources WHERE subject_id=$1 AND ($2::text[] IS NULL OR resource_ref=ANY($2))")
            .bind(subject.0)
        .bind(view.map(|view| view.lexical_visibility.iter().filter(|item| item.object_kind == "resource").map(|item| item.canonical_ref.clone()).collect::<Vec<_>>()))
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?
    {
        nodes.insert(CognitiveRef::Resource(ResourceRef::new(resource)?));
    }
    let mut nodes: Vec<_> = nodes.into_iter().collect();
    nodes.sort_by_key(ToString::to_string);
    Ok(TopologyProjectionInput {
        watermark,
        nodes,
        edges,
    })
}
fn allowed(reference: &CognitiveRef) -> bool {
    matches!(
        reference,
        CognitiveRef::MemoryRevision(_)
            | CognitiveRef::CognitiveSchemaRevision(_)
            | CognitiveRef::EpisodeRevision(_)
            | CognitiveRef::JournalRevision(_)
            | CognitiveRef::Tag(_)
            | CognitiveRef::Entity(_)
            | CognitiveRef::Resource(_)
    )
}

async fn canonical_endpoint_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    reference: CognitiveRef,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<Option<CognitiveRef>> {
    if let Some(view) = view {
        Ok(match reference {
            CognitiveRef::Tag(tag) => view.canonical_tag(tag).map(CognitiveRef::Tag),
            _ => Some(
                view.cognition_for(&reference)
                    .filter(|state| state.object == reference)
                    .map_or(reference.clone(), |state| state.head.clone()),
            ),
        })
    } else {
        crate::tags::canonical_topology_ref_in(tx, subject, reference).await
    }
}
