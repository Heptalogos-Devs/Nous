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
                let tag = CognitiveRef::Tag(TagId(tag.parse().map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InternalFailure,
                        "invalid Tag id",
                    )
                    .with_source(error)
                })?));
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
                let schema_id: Uuid = schema.parse().map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InternalFailure,
                        "invalid schema id",
                    )
                    .with_source(error)
                })?;
                let revision = sqlx::query_scalar::<_, Uuid>(
                        "SELECT h.revision_id FROM cognition_objects o JOIN cognition_revision_headers h ON h.subject_id=o.subject_id AND h.object_kind=o.object_kind AND h.object_id=o.object_id AND h.revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.object_kind='cognitive_schema' AND o.object_id=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal'",
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
                "SELECT rr.from_revision_id,rr.to_revision_id,rr.relation FROM memory_revision_relations rr JOIN cognition_revision_headers l ON l.revision_id=rr.from_revision_id AND l.object_kind='memory' JOIN cognition_objects lo ON lo.subject_id=l.subject_id AND lo.object_kind=l.object_kind AND lo.object_id=l.object_id AND lo.current_revision_id=l.revision_id JOIN cognition_revision_headers r ON r.revision_id=rr.to_revision_id AND r.object_kind='memory' JOIN cognition_objects ro ON ro.subject_id=r.subject_id AND ro.object_kind=r.object_kind AND ro.object_id=r.object_id AND ro.current_revision_id=r.revision_id WHERE lo.subject_id=$1 AND ro.subject_id=$1 AND lo.acceptance_state='accepted' AND ro.acceptance_state='accepted' AND lo.integrity_state='valid' AND ro.integrity_state='valid' AND lo.suppression_state='normal' AND ro.suppression_state='normal' AND lo.purge_state='normal' AND ro.purge_state='normal'",
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
