// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use database_error as db;
use nous_core::*;
use sqlx::{Executor, Postgres, Row, Transaction};

impl AuthorityStore {
    /// Resolve a query exact target once at binding time.  Mutable cognition
    /// object refs are deliberately converted to their current exact revision;
    /// callers retain the returned epoch and must reject a changed head rather
    /// than silently rebinding during execution.
    pub async fn bind_exact_reference(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<(CognitiveRef, Option<i64>, bool)> {
        let (query, id, mutable) = match reference {
            CognitiveRef::Memory(id) => (
                "SELECT current_revision_id,object_epoch FROM memory_objects WHERE subject_id=$1 AND memory_id=$2",
                id.0,
                true,
            ),
            CognitiveRef::MemoryRevision(id) => (
                "SELECT r.memory_revision_id AS current_revision_id,o.object_epoch FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2",
                id.0,
                false,
            ),
            CognitiveRef::Episode(id) => (
                "SELECT current_revision_id,object_epoch FROM episode_objects WHERE subject_id=$1 AND episode_id=$2",
                id.0,
                true,
            ),
            CognitiveRef::EpisodeRevision(id) => (
                "SELECT r.episode_revision_id AS current_revision_id,o.object_epoch FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2",
                id.0,
                false,
            ),
            CognitiveRef::Journal(id) => (
                "SELECT current_revision_id,object_epoch FROM journal_objects WHERE subject_id=$1 AND journal_id=$2",
                id.0,
                true,
            ),
            CognitiveRef::JournalRevision(id) => (
                "SELECT r.journal_revision_id AS current_revision_id,o.object_epoch FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2",
                id.0,
                false,
            ),
            CognitiveRef::CognitiveSchema(id) => (
                "SELECT current_revision_id,object_epoch FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2",
                id.0,
                true,
            ),
            CognitiveRef::CognitiveSchemaRevision(id) => (
                "SELECT r.schema_revision_id AS current_revision_id,s.object_epoch FROM cognitive_schema_revisions r JOIN cognitive_schemas s USING(schema_id) WHERE s.subject_id=$1 AND r.schema_revision_id=$2",
                id.0,
                false,
            ),
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
        };
        let row = sqlx::query(query)
            .bind(subject.0)
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

    #[expect(
        clippy::too_many_lines,
        reason = "subject ownership checks are explicit per cognitive reference owner"
    )]
    async fn reference_in_subject_executor<'e, E>(
        executor: E,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let valid = match reference {
            CognitiveRef::Memory(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM memory_objects WHERE subject_id=$1 AND memory_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::MemoryRevision(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM memory_revisions WHERE subject_id=$1 AND memory_revision_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Episode(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM episode_objects WHERE subject_id=$1 AND episode_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::EpisodeRevision(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE o.subject_id=$1 AND r.episode_revision_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::Journal(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM journal_objects WHERE subject_id=$1 AND journal_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::JournalRevision(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE o.subject_id=$1 AND r.journal_revision_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::CognitiveSchema(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
            CognitiveRef::CognitiveSchemaRevision(id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM cognitive_schema_revisions r JOIN cognitive_schemas s USING(schema_id) WHERE s.subject_id=$1 AND r.schema_revision_id=$2)",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_one(executor)
            .await
            .map_err(db)?,
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
                "SELECT EXISTS(SELECT 1 FROM lexical_bindings b JOIN lexical_visibility v USING(lexical_ref) WHERE v.subject_id=$1 AND b.object_kind='entity' AND b.canonical_ref=$2 AND b.tombstoned_at IS NULL) OR EXISTS(SELECT 1 FROM observation_occurrences WHERE subject_id=$1 AND actor_entity_ref=$2) OR EXISTS(SELECT 1 FROM entity_mentions m JOIN entity_binding_revisions b USING(mention_id) WHERE m.subject_id=$1 AND b.entity_ref=$2 AND b.binding_state='bound') OR EXISTS(SELECT 1 FROM memory_revision_aboutness e JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=$1 AND e.entity_ref=$2) OR EXISTS(SELECT 1 FROM cognitive_schema_revisions r JOIN cognitive_schemas s USING(schema_id) WHERE s.subject_id=$1 AND $2 = ANY(r.aboutness))",
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
}
