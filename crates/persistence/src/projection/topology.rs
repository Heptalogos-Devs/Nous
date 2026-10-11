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

pub struct CognitiveProjectionRows {
    pub authority_watermark: i64,
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
    ) -> Result<TopologyProjectionInput> {
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let result = topology_snapshot_in(&mut tx, subject, memory_enabled).await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    /// Topology, text representations and curve memberships from one Authority
    /// snapshot. Embeddings are resolved by the serving adapter after this read.
    pub async fn cognitive_projection_rows(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        budget: crate::EpisodeTextBudget,
    ) -> Result<CognitiveProjectionRows> {
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let authority_watermark: i64 =
            sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1")
                .bind(subject.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        let topology = topology_snapshot_in(&mut tx, subject, memory_enabled).await?;
        let mut sources = crate::projection::text::material_sources(&mut tx, subject, None).await?;
        if memory_enabled {
            sources.extend(crate::projection::text::memory_sources(&mut tx, subject, None).await?);
            sources.extend(
                crate::projection::text::longitudinal_sources(&mut tx, subject, budget, None)
                    .await?,
            );
        }
        sources.sort_by_key(|s| s.reference.to_string());
        tx.commit().await.map_err(db)?;
        Ok(CognitiveProjectionRows {
            authority_watermark,
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
) -> Result<TopologyProjectionInput> {
    let watermark: i64 =
        sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
    let mut nodes = HashSet::new();
    let mut edges = Vec::new();
    if memory_enabled {
        crate::projection::longitudinal::append(tx, subject, &mut nodes, &mut edges).await?;
        let sources = crate::projection::text::memory_sources(tx, subject, None).await?;
        for source in sources {
            nodes.insert(source.reference.clone());
            for tag in source.tag_ids {
                let tag = CognitiveRef::Tag(TagId(
                    tag.parse()
                        .map_err(|_| Error::Infrastructure("invalid Tag id".into()))?,
                ));
                nodes.insert(tag.clone());
                let root = format!("structure:tag:{}:{}", source.reference, tag);
                edges.push(link(
                    source.reference.clone(),
                    tag.clone(),
                    "tag_attachment",
                    root.clone(),
                ));
                edges.push(link(tag, source.reference.clone(), "tag_attachment", root));
            }
            for schema in source.schema_ids {
                let schema_id: Uuid = schema
                    .parse()
                    .map_err(|_| Error::Infrastructure("invalid schema id".into()))?;
                let revision = sqlx::query_scalar::<_, Uuid>(
                        "SELECT current_revision_id FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' AND purge_state='normal'",
                    )
                    .bind(subject.0)
                    .bind(schema_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(db)?;
                let Some(revision) = revision else { continue };
                let schema =
                    CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(revision));
                nodes.insert(schema.clone());
                let root = format!("structure:schema:{}:{}", source.reference, schema);
                edges.push(link(
                    source.reference.clone(),
                    schema.clone(),
                    "schema_support",
                    root.clone(),
                ));
                edges.push(link(
                    schema,
                    source.reference.clone(),
                    "schema_support",
                    root,
                ));
            }
            for entity in source.entity_refs {
                let entity = CognitiveRef::Entity(EntityRef::new(entity)?);
                nodes.insert(entity.clone());
                let root = format!("structure:aboutness:{}:{}", source.reference, entity);
                edges.push(link(
                    source.reference.clone(),
                    entity.clone(),
                    "aboutness",
                    root.clone(),
                ));
                edges.push(link(entity, source.reference.clone(), "aboutness", root));
            }
        }

        let association_rows = sqlx::query(
                "SELECT association_evidence_id,from_ref_kind,from_ref,to_ref_kind,to_ref,basis_class,relation_kind,polarity FROM association_evidence WHERE subject_id=$1 AND revoked_at IS NULL",
            )
            .bind(subject.0)
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
            let Some(from) = crate::tags::canonical_topology_ref_in(tx, subject, from).await?
            else {
                continue;
            };
            let Some(to) = crate::tags::canonical_topology_ref_in(tx, subject, to).await? else {
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
                "SELECT rr.from_revision_id,rr.to_revision_id,rr.relation FROM memory_revision_relations rr JOIN memory_revisions l ON l.memory_revision_id=rr.from_revision_id JOIN memory_revisions r ON r.memory_revision_id=rr.to_revision_id JOIN memory_objects lo ON lo.memory_id=l.memory_id JOIN memory_objects ro ON ro.memory_id=r.memory_id WHERE lo.subject_id=$1 AND ro.subject_id=$1 AND lo.current_revision_id=l.memory_revision_id AND ro.current_revision_id=r.memory_revision_id AND lo.acceptance_state='accepted' AND ro.acceptance_state='accepted' AND lo.integrity_state='valid' AND ro.integrity_state='valid' AND lo.suppression_state='normal' AND ro.suppression_state='normal' AND lo.purge_state='normal' AND ro.purge_state='normal'",
            )
            .bind(subject.0)
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
        sqlx::query_scalar::<_, String>("SELECT resource_ref FROM resources WHERE subject_id=$1")
            .bind(subject.0)
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
