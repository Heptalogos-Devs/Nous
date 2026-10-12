// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn revision(
        &self,
        subject: SubjectId,
        revision: MemoryRevisionId,
    ) -> Result<MemoryView> {
        self.full_memory_views(subject, &[revision.0])
            .await?
            .remove(&revision.0)
            .ok_or_else(|| Error::NotFound("memory not found".into()))
    }

    pub async fn memory(
        &self,
        subject: SubjectId,
        memory_id: MemoryId,
        revision: Option<MemoryRevisionId>,
    ) -> Result<MemoryView> {
        let selected: Option<Uuid> = sqlx::query_scalar("SELECT r.memory_revision_id FROM memory_objects o JOIN memory_revisions r ON r.memory_id=o.memory_id AND r.memory_revision_id=COALESCE($3,o.current_revision_id) WHERE o.subject_id=$1 AND o.memory_id=$2")
            .bind(subject.0).bind(memory_id.0).bind(revision.map(|id| id.0)).fetch_optional(self.store.pool()).await.map_err(db)?;
        self.revision(
            subject,
            MemoryRevisionId(selected.ok_or_else(|| Error::NotFound("memory not found".into()))?),
        )
        .await
    }

    pub async fn memories(
        &self,
        subject: SubjectId,
        status: &str,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<MemoryView>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("page limit is out of bounds".into()));
        }
        self.store.require_subject(subject).await?;
        if !matches!(status, "" | "accepted" | "withdrawn") {
            return Err(Error::Invalid("invalid memory acceptance status".into()));
        }
        let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT current_revision_id FROM memory_objects WHERE subject_id=$1 AND ($2='' OR acceptance_state=$2) AND ($3::uuid IS NULL OR memory_id>$3) ORDER BY memory_id LIMIT $4")
            .bind(subject.0).bind(status).bind(after).bind(limit+1).fetch_all(self.store.pool()).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let mut views = self.full_memory_views(subject, &ids).await?;
        let items = ids
            .into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("memory page changed while reading".into()))
            })
            .collect::<Result<_>>()?;
        Ok((items, more))
    }

    async fn full_memory_views(
        &self,
        subject: SubjectId,
        revisions: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, MemoryView>> {
        let policy =
            resolve_accessibility_policy(&self.configuration.snapshot_for_subject(subject)?)?;
        self.memory_views(
            subject,
            revisions,
            &policy,
            None,
            true,
            self.cognition.now(subject),
        )
        .await
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
                _ => return Err(Error::Infrastructure("evidence locator union is invalid".into())),
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
        after: Option<Uuid>,
    ) -> Result<(Vec<MemoryView>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("page limit is out of bounds".into()));
        }
        self.store
            .validate_reference(subject, &CognitiveRef::Memory(memory_id))
            .await?;
        let after_no = if let Some(after) = after {
            Some(sqlx::query_scalar::<_, i32>("SELECT revision_no FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2 AND memory_revision_id=$3")
                .bind(subject.0).bind(memory_id.0).bind(after).fetch_optional(self.store.pool()).await.map_err(db)?
                .ok_or_else(|| Error::Invalid("invalid memory history cursor".into()))?)
        } else {
            None
        };
        let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2 AND ($3::int IS NULL OR revision_no>$3) ORDER BY revision_no LIMIT $4")
            .bind(subject.0).bind(memory_id.0).bind(after_no).bind(limit+1).fetch_all(self.store.pool()).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let mut views = self.full_memory_views(subject, &ids).await?;
        let items = ids
            .into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("memory history changed while reading".into()))
            })
            .collect::<Result<_>>()?;
        Ok((items, more))
    }
}
