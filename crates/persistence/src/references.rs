// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use database_error as db;
use nous_core::*;
use sqlx::{Executor, Postgres, Row, Transaction};

/// Project a typed Cognition reference into the shared physical identity.
/// `is_object` selects the mutable object's current head; `false` keeps an
/// exact immutable revision identity.
pub(crate) fn cognition_ref_parts(
    reference: &CognitiveRef,
) -> Option<(&'static str, uuid::Uuid, bool)> {
    Some(match reference {
        CognitiveRef::Memory(id) => ("memory", id.0, true),
        CognitiveRef::MemoryRevision(id) => ("memory", id.0, false),
        CognitiveRef::Episode(id) => ("episode", id.0, true),
        CognitiveRef::EpisodeRevision(id) => ("episode", id.0, false),
        CognitiveRef::Journal(id) => ("journal", id.0, true),
        CognitiveRef::JournalRevision(id) => ("journal", id.0, false),
        CognitiveRef::CognitiveSchema(id) => ("cognitive_schema", id.0, true),
        CognitiveRef::CognitiveSchemaRevision(id) => ("cognitive_schema", id.0, false),
        _ => return None,
    })
}

impl AuthorityStore {
    /// Clear context bodies that directly depended on purged owner refs while
    /// retaining an ended shell for existing longitudinal references.
    pub async fn purge_work_contexts_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<()> {
        if references.is_empty() {
            return Ok(());
        }
        let parts = references.iter().map(reference_parts).collect::<Vec<_>>();
        let (kinds, values): (Vec<_>, Vec<_>) = parts.into_iter().unzip();
        let affected: Vec<uuid::Uuid> = sqlx::query_scalar(
            r#"
WITH targets(ref_kind,ref_value) AS (
    SELECT * FROM unnest($2::text[],$3::text[])
)
SELECT DISTINCT wc.work_context_id
FROM work_contexts wc
JOIN work_context_refs wcr USING(work_context_id)
JOIN targets requested USING(ref_kind,ref_value)
WHERE wc.subject_id=$1
ORDER BY wc.work_context_id
"#,
        )
        .bind(subject.0)
        .bind(&kinds)
        .bind(&values)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
        for id in affected {
            let context = CognitiveRef::WorkContext(nous_core::WorkContextId(id));
            Self::tombstone_identity_refs_in(tx, subject, std::slice::from_ref(&context)).await?;
            Self::purge_owner_history_in(tx, subject, std::slice::from_ref(&context)).await?;
        }
        sqlx::query(
            r#"
WITH targets(ref_kind,ref_value) AS (
    SELECT * FROM unnest($2::text[],$3::text[])
)
UPDATE work_contexts wc SET
    state='ended',purpose='Purged context',context_text='',
    unresolved_questions='{}',constraints='{}',resume_conditions='{}',
    budget_summary='{}',revision=revision+1,
    ended_at=COALESCE(ended_at,clock_timestamp()),updated_at=clock_timestamp()
WHERE wc.subject_id=$1 AND EXISTS (
    SELECT 1 FROM work_context_refs wcr
    JOIN targets requested USING(ref_kind,ref_value)
    WHERE wcr.work_context_id=wc.work_context_id
)
"#,
        )
        .bind(subject.0)
        .bind(&kinds)
        .bind(&values)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        sqlx::query(
            r#"
WITH targets(ref_kind,ref_value) AS (
    SELECT * FROM unnest($2::text[],$3::text[])
), affected(work_context_id) AS (
    SELECT DISTINCT wc.work_context_id
    FROM work_contexts wc
    JOIN work_context_refs wcr USING(work_context_id)
    JOIN targets requested USING(ref_kind,ref_value)
    WHERE wc.subject_id=$1
)
DELETE FROM work_context_refs wcr USING affected
WHERE wcr.work_context_id=affected.work_context_id
"#,
        )
        .bind(subject.0)
        .bind(kinds)
        .bind(values)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Resolve a query exact target once at binding time.  Mutable cognition
    /// object refs are deliberately converted to their current exact revision;
    /// callers retain the returned epoch and must reject a changed head rather
    /// than silently rebinding during execution.
    pub async fn bind_exact_reference(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<(CognitiveRef, Option<i64>, bool)> {
        // All four cognition families share a single immutable revision
        // identity and current-head table. Keep the kind supplied by the
        // caller in the predicate so UUID collisions across kinds can never
        // resolve to the wrong owner.
        let (kind, id, mutable) = match cognition_ref_parts(reference) {
            Some(parts) => parts,
            None => match reference {
                CognitiveRef::Tag(tag) => {
                    return Ok((
                        CognitiveRef::Tag(self.canonical_tag_id(subject, *tag).await?),
                        None,
                        false,
                    ));
                }
                _ => {
                    self.validate_reference(subject, reference).await?;
                    return Ok((reference.clone(), None, false));
                }
            },
        };
        let row = sqlx::query(if mutable {
            "SELECT current_revision_id,object_epoch FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3"
        } else {
            "SELECT h.revision_id AS current_revision_id,o.object_epoch FROM cognition_revision_headers h JOIN cognition_objects o USING(subject_id,object_kind,object_id) WHERE h.subject_id=$1 AND h.object_kind=$2 AND h.revision_id=$3"
        })
            .bind(subject.0)
            .bind(kind)
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Identity,
                    nous_core::ErrorCode::NotFound,
                    "cognition exact target not found",
                )
            })?;
        let revision = row.try_get("current_revision_id").map_err(db)?;
        let exact = match reference {
            CognitiveRef::Memory(_) => CognitiveRef::MemoryRevision(MemoryRevisionId(revision)),
            CognitiveRef::Episode(_) => CognitiveRef::EpisodeRevision(EpisodeRevisionId(revision)),
            CognitiveRef::Journal(_) => CognitiveRef::JournalRevision(JournalRevisionId(revision)),
            CognitiveRef::CognitiveSchema(_) => {
                CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(revision))
            }
            _ => reference.clone(),
        };
        Ok((
            exact,
            Some(row.try_get("object_epoch").map_err(db)?),
            mutable,
        ))
    }

    pub async fn require_subject(&self, subject: SubjectId) -> Result<()> {
        if self.subject_exists(subject).await? {
            Ok(())
        } else {
            Err(Error::new(
                nous_core::ErrorOwner::Identity,
                nous_core::ErrorCode::NotFound,
                "subject not found",
            ))
        }
    }

    pub async fn reference_in_subject(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<bool> {
        Self::reference_in_subject_executor(self.pool(), subject, reference).await
    }

    pub async fn reference_in_subject_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<bool> {
        Self::reference_in_subject_executor(&mut **tx, subject, reference).await
    }

    async fn reference_in_subject_executor<'e, E>(
        executor: E,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let valid = if let Some((kind, id, object)) = cognition_ref_parts(reference) {
            sqlx::query_scalar::<_, bool>(
                "SELECT CASE WHEN $4 THEN EXISTS(SELECT 1 FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3) ELSE EXISTS(SELECT 1 FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind=$2 AND revision_id=$3) END",
            )
            .bind(subject.0)
            .bind(kind)
            .bind(id)
            .bind(object)
            .fetch_one(executor)
            .await
            .map_err(db)?
        } else {
            match reference {
            CognitiveRef::CognitiveSeedVersion(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM cognitive_seed_versions WHERE subject_id=$1 AND seed_version_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Artifact(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM artifacts WHERE subject_id=$1 AND artifact_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::SourceRegion(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM source_regions WHERE subject_id=$1 AND source_region_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::DerivedRepresentation(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM derived_representations WHERE subject_id=$1 AND derived_representation_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::DerivedRegion(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM derived_regions WHERE subject_id=$1 AND derived_region_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Tag(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM tags WHERE subject_id=$1 AND tag_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Resource(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM resources WHERE subject_id=$1 AND resource_ref=$2)",
            )
            .bind(subject.0)
            .bind(id.as_str())
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::ExternalObject(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM observation_occurrences WHERE subject_id=$1 AND external_object_ref=$2)",
            )
            .bind(subject.0)
            .bind(id.as_str())
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Occurrence(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Entity(id) => {
                EntityRef::new(id.as_str())?;
                sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM lexical_bindings b JOIN lexical_visibility v USING(lexical_ref) WHERE v.subject_id=$1 AND b.object_kind='entity' AND b.canonical_ref=$2 AND b.tombstoned_at IS NULL) OR EXISTS(SELECT 1 FROM observation_occurrences WHERE subject_id=$1 AND actor_entity_ref=$2) OR EXISTS(SELECT 1 FROM entity_mentions m JOIN entity_binding_revisions b USING(mention_id) WHERE m.subject_id=$1 AND b.entity_ref=$2 AND b.binding_state='bound') OR EXISTS(SELECT 1 FROM memory_revision_aboutness e JOIN cognition_revision_headers h ON h.revision_id=e.memory_revision_id AND h.object_kind='memory' WHERE h.subject_id=$1 AND e.entity_ref=$2) OR EXISTS(SELECT 1 FROM cognitive_schema_revisions r JOIN cognition_revision_headers h ON h.revision_id=r.schema_revision_id AND h.object_kind='cognitive_schema' WHERE h.subject_id=$1 AND $2 = ANY(r.aboutness))",
                )
                .bind(subject.0)
                .bind(id.as_str())
                .fetch_one(executor)
                .await
                .map_err(db)?
            }
            CognitiveRef::Subject(id) => id == &subject,
            CognitiveRef::WorkContext(id) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_contexts WHERE subject_id=$1 AND work_context_id=$2)").bind(subject.0).bind(id.0).fetch_one(executor).await.map_err(db)?,
            CognitiveRef::Association(id) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM association_evidence WHERE subject_id=$1 AND association_evidence_id=$2)").bind(subject.0).bind(id.0).fetch_one(executor).await.map_err(db)?,
            CognitiveRef::Session(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM cognitive_sessions WHERE subject_id=$1 AND session_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Memory(_)
            | CognitiveRef::MemoryRevision(_)
            | CognitiveRef::Episode(_)
            | CognitiveRef::EpisodeRevision(_)
            | CognitiveRef::Journal(_)
            | CognitiveRef::JournalRevision(_)
            | CognitiveRef::CognitiveSchema(_)
            | CognitiveRef::CognitiveSchemaRevision(_) => {
                unreachable!("Cognition refs use the shared header existence query")
            }
        }
        };
        Ok(valid)
    }

    pub async fn validate_reference(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<()> {
        self.require_subject(subject).await?;
        if self.reference_in_subject(subject, reference).await? {
            Ok(())
        } else {
            Err(Error::new(
                nous_core::ErrorOwner::Identity,
                nous_core::ErrorCode::NotFound,
                "cognitive reference not found",
            ))
        }
    }

    /// Verify that cached document bindings still target live purge owners while
    /// the caller holds the Subject row lock used by purge checkpoints.
    pub async fn validate_not_purging_refs_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<()> {
        let parts = references.iter().map(reference_parts).collect::<Vec<_>>();
        let (kinds, values): (Vec<_>, Vec<_>) = parts.into_iter().unzip();
        let unsafe_source: bool = sqlx::query_scalar(
            r#"
WITH requested(ref_kind,ref_value) AS (
    SELECT * FROM unnest($2::text[],$3::text[])
), owners(ref_kind,ref_value,purge_state) AS (
    SELECT object_kind,object_id::text,purge_state
      FROM cognition_objects WHERE subject_id=$1
    UNION ALL
    SELECT h.object_kind || '_revision',h.revision_id::text,o.purge_state
      FROM cognition_revision_headers h
      JOIN cognition_objects o USING(subject_id,object_kind,object_id)
     WHERE o.subject_id=$1
)
SELECT EXISTS(
    SELECT 1 FROM requested r
    LEFT JOIN owners o USING(ref_kind,ref_value)
    WHERE r.ref_kind IN (
        'memory','memory_revision','cognitive_schema','cognitive_schema_revision',
        'episode','episode_revision','journal','journal_revision'
    ) AND o.purge_state IS DISTINCT FROM 'normal'
)
"#,
        )
        .bind(subject.0)
        .bind(kinds)
        .bind(values)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
        if unsafe_source {
            return Err(Error::new(
                ErrorOwner::Material,
                ErrorCode::StaleRevision,
                "embedding source changed or is being purged",
            ));
        }
        Ok(())
    }
}
