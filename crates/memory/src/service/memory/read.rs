// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn revision(
        &self,
        subject: SubjectId,
        revision: MemoryRevisionId,
    ) -> Result<MemoryView> {
        let memory: Uuid = sqlx::query_scalar(
            "SELECT memory_id FROM memory_revisions WHERE subject_id=$1 AND memory_revision_id=$2",
        )
        .bind(subject.0)
        .bind(revision.0)
        .fetch_one(self.store.pool())
        .await
        .map_err(db)?;
        self.memory(subject, MemoryId(memory), Some(revision)).await
    }

    pub async fn memory(
        &self,
        subject: SubjectId,
        memory_id: MemoryId,
        revision: Option<MemoryRevisionId>,
    ) -> Result<MemoryView> {
        let row = sqlx::query("SELECT o.memory_id,o.subject_id,o.cognitive_role,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,o.accessibility_mode,o.created_at,r.memory_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.formation_mode,r.grounding_occurrence_id,r.semantic_role,r.title,r.representation_text,r.epistemic_class,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,r.producer_signature_id FROM memory_objects o JOIN memory_revisions r ON r.memory_id=o.memory_id AND r.memory_revision_id=COALESCE($3,o.current_revision_id) WHERE o.subject_id=$1 AND o.memory_id=$2")
            .bind(subject.0)
            .bind(memory_id.0)
            .bind(revision.map(|value| value.0))
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("memory not found".into()))?;
        let revision_id = MemoryRevisionId(row.try_get("memory_revision_id").map_err(db)?);
        let (object, revision_value) = decode_memory_row(&row, subject)?;
        let basis = self.load_basis(revision_id).await?;
        let aboutness = sqlx::query_scalar::<_, String>("SELECT entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=$1 ORDER BY entity_ref")
            .bind(revision_id.0).fetch_all(self.store.pool()).await.map_err(db)?.into_iter().map(EntityRef::new).collect::<Result<Vec<_>>>()?;
        let tags = sqlx::query_scalar::<_, Uuid>(
            "SELECT tag_id FROM memory_revision_tags WHERE memory_revision_id=$1 ORDER BY tag_id",
        )
        .bind(revision_id.0)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?
        .into_iter()
        .map(TagId)
        .collect();
        let relation_rows = sqlx::query("SELECT from_revision_id,to_revision_id,relation,created_at FROM memory_revision_relations WHERE from_revision_id=$1 OR to_revision_id=$1 ORDER BY created_at,from_revision_id,to_revision_id LIMIT 257")
            .bind(revision_id.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let relations_truncated = relation_rows.len() > 256;
        let relations = relation_rows
            .into_iter()
            .take(256)
            .map(|row| {
                Ok(MemoryRevisionRelation {
                    from_revision_id: MemoryRevisionId(
                        row.try_get("from_revision_id").map_err(db)?,
                    ),
                    to_revision_id: MemoryRevisionId(row.try_get("to_revision_id").map_err(db)?),
                    relation: parse_enum(row.try_get("relation").map_err(db)?, "memory relation")?,
                    created_at: row.try_get("created_at").map_err(db)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let accessibility_level = self
            .accessibility_level(subject, memory_id, self.cognition.now(subject))
            .await?;
        let temporal_evidence = self.temporal_evidence(revision_id).await?;
        let source_classes = self.source_classes(revision_id).await?;
        Ok(MemoryView {
            object,
            revision: revision_value,
            basis,
            aboutness,
            tags,
            relations,
            relations_truncated,
            accessibility_level,
            temporal_evidence,
            source_classes,
        })
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
    ) -> Result<Vec<MemoryView>> {
        if !self
            .store
            .reference_in_subject(subject, &CognitiveRef::Memory(memory_id))
            .await?
        {
            return Err(Error::NotFound("memory not found".into()));
        }
        let ids = sqlx::query_scalar::<_, Uuid>("SELECT memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2 ORDER BY revision_no")
            .bind(subject.0).bind(memory_id.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result = Vec::new();
        for id in ids {
            result.push(
                self.memory(subject, memory_id, Some(MemoryRevisionId(id)))
                    .await?,
            );
        }
        Ok(result)
    }
}
