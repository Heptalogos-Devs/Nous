// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn revision(
        &self,
        subject: SubjectId,
        revision: MemoryRevisionId,
    ) -> Result<MemoryView> {
        let reference = CognitiveRef::MemoryRevision(revision);
        let context = self.current_read_context(subject).await?;
        match self
            .read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
        {
            Some(CognitionView::Memory(view)) => Ok(view),
            _ => Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::NotFound,
                "memory revision not found",
            )),
        }
    }

    pub async fn memory(
        &self,
        subject: SubjectId,
        memory_id: MemoryId,
        revision: Option<MemoryRevisionId>,
    ) -> Result<MemoryView> {
        let reference = revision.map_or(CognitiveRef::Memory(memory_id), |revision| {
            CognitiveRef::MemoryRevision(revision)
        });
        let context = self.current_read_context(subject).await?;
        match self
            .read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
        {
            Some(CognitionView::Memory(view)) if view.header.object_id() == memory_id.0 => Ok(view),
            _ => Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::NotFound,
                "memory not found",
            )),
        }
    }

    pub(in crate::service) async fn load_basis(
        &self,
        revision: MemoryRevisionId,
    ) -> Result<Vec<RevisionBasis>> {
        let mut basis = Vec::new();
        for row in sqlx::query("SELECT 'evidence'::text AS basis_kind,occurrence_id,source_region_id,derived_representation_id,derived_region_id,basis_role,epistemic_relation FROM memory_revision_evidence WHERE memory_revision_id=$1 ORDER BY evidence_no")
            .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)? {
            basis.push(decode_revision_basis(ErrorOwner::Memory, &row)?);
        }
        for row in sqlx::query("SELECT target_ref_kind AS basis_kind,target_ref AS basis_ref,basis_role,epistemic_relation FROM memory_revision_dependencies WHERE memory_revision_id=$1 ORDER BY target_ref_kind,target_ref")
            .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)? {
            basis.push(decode_revision_basis(ErrorOwner::Memory, &row)?);
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
                ErrorOwner::Memory,
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
}
