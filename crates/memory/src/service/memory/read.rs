// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn revision(
        &self,
        subject: SubjectId,
        revision: MemoryRevisionId,
    ) -> Result<MemoryView> {
        self.memories_by_revision(subject, &[revision])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::NotFound,
                    "memory revision not found",
                )
            })
    }

    pub async fn memory(
        &self,
        subject: SubjectId,
        memory_id: MemoryId,
        revision: Option<MemoryRevisionId>,
    ) -> Result<MemoryView> {
        let view = if let Some(revision) = revision {
            self.memories_by_revision(subject, &[revision])
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::NotFound,
                        "memory not found",
                    )
                })?
        } else {
            self.memory_views_for_references(subject, &[CognitiveRef::Memory(memory_id)])
                .await?
                .remove(&CognitiveRef::Memory(memory_id))
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::NotFound,
                        "memory not found",
                    )
                })?
        };
        if view.object.memory_id != memory_id {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::NotFound,
                "memory not found",
            ));
        }
        Ok(view)
    }

    pub(in crate::service) async fn load_basis(
        &self,
        revision: MemoryRevisionId,
    ) -> Result<Vec<RevisionBasis>> {
        let mut basis = Vec::new();
        for row in sqlx::query("SELECT occurrence_id,source_region_id,derived_representation_id,derived_region_id,basis_role,epistemic_relation FROM memory_revision_evidence WHERE memory_revision_id=$1 ORDER BY evidence_no")
            .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)? {
            let locator = match (
                row.try_get::<Option<Uuid>, _>("source_region_id").map_err(db)?,
                row.try_get::<Option<Uuid>, _>("derived_representation_id").map_err(db)?,
                row.try_get::<Option<Uuid>, _>("derived_region_id").map_err(db)?,
            ) {
                (Some(id), None, None) => EvidenceLocator::SourceRegion(nous_core::SourceRegionId(id)),
                (None, Some(id), None) => EvidenceLocator::DerivedRepresentation(nous_core::DerivedRepresentationId(id)),
                (None, None, Some(id)) => EvidenceLocator::DerivedRegion(nous_core::DerivedRegionId(id)),
                (None, None, None) => EvidenceLocator::WholeOccurrence,
                _ => return Err(Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InternalFailure, "evidence locator union is invalid")),
            };
            basis.push(RevisionBasis::Evidence(EvidenceRef {
                    epistemic_relation: parse_epistemic_relation(row.try_get("epistemic_relation").map_err(db)?)?, occurrence_id: OccurrenceId(row.try_get("occurrence_id").map_err(db)?), locator, basis_role: parse_enum(row.try_get("basis_role").map_err(db)?, "basis role")? }));
        }
        for row in sqlx::query("SELECT target_ref_kind,target_ref,basis_role,epistemic_relation FROM memory_revision_dependencies WHERE memory_revision_id=$1 ORDER BY target_ref_kind,target_ref")
            .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)? {
            basis.push(RevisionBasis::CognitionDependency(CognitionDependency {
                    epistemic_relation: parse_epistemic_relation(row.try_get("epistemic_relation").map_err(db)?)?, target_revision: parse_reference(&row.try_get::<String, _>("target_ref_kind").map_err(db)?, &row.try_get::<String, _>("target_ref").map_err(db)?)?, basis_role: parse_enum(row.try_get("basis_role").map_err(db)?, "basis role")? }));
        }
        Ok(basis)
    }

    pub(in crate::service) async fn temporal_evidence(
        &self,
        revision: MemoryRevisionId,
    ) -> Result<TemporalEvidence> {
        let rows = sqlx::query("SELECT o.occurred_time_kind,o.occurred_time_start,o.occurred_time_end,o.observed_at FROM memory_revision_evidence e JOIN observation_occurrences o USING(occurrence_id) WHERE e.memory_revision_id=$1 ORDER BY o.observed_at")
            .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut occurred = Vec::new();
        let mut observed_at = None;
        for row in rows {
            occurred.push(temporal_from_columns(
                row.try_get("occurred_time_kind").map_err(db)?,
                row.try_get("occurred_time_start").map_err(db)?,
                row.try_get("occurred_time_end").map_err(db)?,
            )?);
            observed_at = Some(row.try_get("observed_at").map_err(db)?);
        }
        Ok(TemporalEvidence {
            occurred,
            observed_at,
        })
    }

    pub async fn memory_history(
        &self,
        subject: SubjectId,
        memory_id: MemoryId,
        limit: i64,
        after: Option<MemoryRevisionId>,
    ) -> Result<Vec<MemoryView>> {
        let after_revision = match after {
            Some(revision) => sqlx::query_scalar::<_, i32>(
                "SELECT revision_no FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2 AND memory_revision_id=$3",
            )
            .bind(subject.0).bind(memory_id.0).bind(revision.0)
            .fetch_optional(self.store.pool()).await.map_err(db)?
            .ok_or_else(|| Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InvalidRequest, "history cursor does not belong to this memory"))?,
            None => 0,
        };
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2 AND revision_no>$3 ORDER BY revision_no LIMIT $4",
        )
        .bind(subject.0).bind(memory_id.0).bind(after_revision).bind(limit)
        .fetch_all(self.store.pool()).await.map_err(db)?;
        if ids.is_empty() {
            // Distinguish a finished page from a nonexistent or inaccessible object.
            self.memory(subject, memory_id, None).await?;
        }
        self.memories_by_revision(
            subject,
            &ids.into_iter().map(MemoryRevisionId).collect::<Vec<_>>(),
        )
        .await
    }
}
