//! Cognition lifecycle transactions; immutable content and family validation stay with their owners.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::schema::SchemaView;
use super::*;

fn object_key(target: &CognitiveRef) -> Result<(&'static str, Uuid)> {
    match target {
        CognitiveRef::Memory(id) => Ok(("memory", id.0)),
        CognitiveRef::CognitiveSchema(id) => Ok(("schema", id.0)),
        CognitiveRef::Episode(id) => Ok(("episode", id.0)),
        CognitiveRef::Journal(id) => Ok(("journal", id.0)),
        _ => Err(Error::Invalid(
            "lifecycle requires a cognition object".into(),
        )),
    }
}

fn object_lock(kind: &str) -> &'static str {
    match kind {
        "memory" => {
            "SELECT current_revision_id,object_epoch,acceptance_state,suppression_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE"
        }
        "schema" => {
            "SELECT current_revision_id,object_epoch,acceptance_state,suppression_state FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2 FOR UPDATE"
        }
        "episode" => {
            "SELECT current_revision_id,object_epoch,acceptance_state,suppression_state FROM episode_objects WHERE subject_id=$1 AND episode_id=$2 FOR UPDATE"
        }
        "journal" => {
            "SELECT current_revision_id,object_epoch,acceptance_state,suppression_state FROM journal_objects WHERE subject_id=$1 AND journal_id=$2 FOR UPDATE"
        }
        _ => unreachable!("validated cognition kind"),
    }
}

impl MemoryService {
    async fn transition_cognition(
        &self,
        subject: SubjectId,
        target: CognitiveRef,
        operation: OperationId,
        expected: i64,
        action: LifecycleAction,
    ) -> Result<Uuid> {
        let (kind, id) = object_key(&target)?;
        let (field, from, to) = action.transition();
        let digest = operation_digest(
            "cognition_lifecycle",
            subject,
            &(target, expected, field, from, to),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation, "cognition_lifecycle", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                return receipt.result_revision.ok_or_else(|| {
                    Error::Infrastructure("lifecycle receipt has no exact revision".into())
                });
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query(object_lock(kind))
            .bind(subject.0)
            .bind(id)
            .fetch_optional(&mut **mutation.tx())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("cognition not found".into()))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        let revision: Uuid = row.try_get("current_revision_id").map_err(db)?;
        fence_epoch(epoch, expected)?;
        let current: String = row.try_get(field).map_err(db)?;
        if kind == "memory" && current == to {
            mutation
                .commit(kind, Some(&id.to_string()), Some(revision), Some(epoch))
                .await?;
            return Ok(revision);
        }
        require_transition(&current, from)?;
        if kind == "episode" && matches!(action, LifecycleAction::Reaccept) {
            episode::validate_episode_reaccept_in(mutation.tx(), subject, EpisodeId(id)).await?;
        }
        let update = match kind {
            "memory" => {
                "UPDATE memory_objects SET suppression_state=CASE WHEN $3='suppression_state' THEN $4 ELSE suppression_state END,acceptance_state=CASE WHEN $3='acceptance_state' THEN $4 ELSE acceptance_state END,object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2"
            }
            "schema" => {
                "UPDATE cognitive_schemas SET suppression_state=CASE WHEN $3='suppression_state' THEN $4 ELSE suppression_state END,acceptance_state=CASE WHEN $3='acceptance_state' THEN $4 ELSE acceptance_state END,object_epoch=object_epoch+1 WHERE subject_id=$1 AND schema_id=$2"
            }
            "episode" => {
                "UPDATE episode_objects SET suppression_state=CASE WHEN $3='suppression_state' THEN $4 ELSE suppression_state END,acceptance_state=CASE WHEN $3='acceptance_state' THEN $4 ELSE acceptance_state END,object_epoch=object_epoch+1 WHERE subject_id=$1 AND episode_id=$2"
            }
            "journal" => {
                "UPDATE journal_objects SET suppression_state=CASE WHEN $3='suppression_state' THEN $4 ELSE suppression_state END,acceptance_state=CASE WHEN $3='acceptance_state' THEN $4 ELSE acceptance_state END,object_epoch=object_epoch+1 WHERE subject_id=$1 AND journal_id=$2"
            }
            _ => unreachable!("validated cognition kind"),
        };
        sqlx::query(update)
            .bind(subject.0)
            .bind(id)
            .bind(field)
            .bind(to)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        if matches!(kind, "memory" | "schema") {
            let (revision_kind, revisions) =
                cognition_revisions_in(mutation.tx(), subject, kind, id).await?;
            self.cognition
                .forget_resident_cognition_in(mutation.tx(), subject, revision_kind, &revisions)
                .await?;
        }
        self.invalidate_cognition_change(
            &mut mutation,
            subject,
            kind,
            id,
            "source_lifecycle_changed",
        )
        .await?;
        mutation
            .commit(kind, Some(&id.to_string()), Some(revision), Some(epoch + 1))
            .await?;
        Ok(revision)
    }

    async fn erase_cognition(
        &self,
        subject: SubjectId,
        target: CognitiveRef,
        operation: OperationId,
        expected: i64,
    ) -> Result<()> {
        let (kind, id) = object_key(&target)?;
        let digest = operation_digest("cognition_purge", subject, &(target, expected))?;
        let mut mutation = match self
            .start_mutation(subject, operation, "cognition_purge", &digest)
            .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query(object_lock(kind))
            .bind(subject.0)
            .bind(id)
            .fetch_optional(&mut **mutation.tx())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("cognition not found".into()))?;
        fence_epoch(row.try_get("object_epoch").map_err(db)?, expected)?;
        self.invalidate_cognition_change(&mut mutation, subject, kind, id, "basis_purged")
            .await?;
        let (revision_kind, revisions) =
            cognition_revisions_in(mutation.tx(), subject, kind, id).await?;
        let mut workflow_refs = revisions.clone();
        workflow_refs.push(id.to_string());
        AuthorityStore::purge_model_results_in(mutation.tx(), subject, &workflow_refs).await?;
        self.cognition
            .purge_cognition_refs_in(mutation.tx(), subject, revision_kind, &revisions)
            .await?;
        sqlx::query("UPDATE cognitive_schema_evidence_links SET revoked_at=COALESCE(revoked_at,$4) WHERE subject_id=$1 AND basis_kind=$2 AND basis_ref=ANY($3::text[]) AND revoked_at IS NULL")
                .bind(subject.0).bind(revision_kind).bind(&revisions).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND ((from_ref_kind=$2 AND from_ref=ANY($3::text[])) OR (to_ref_kind=$2 AND to_ref=ANY($3::text[])) OR association_evidence_id IN (SELECT association_evidence_id FROM association_evidence_basis WHERE basis_kind=$2 AND basis_ref=ANY($3::text[])))")
                .bind(subject.0).bind(revision_kind).bind(&revisions).execute(&mut **mutation.tx()).await.map_err(db)?;
        let delete = match kind {
            "memory" => "DELETE FROM memory_objects WHERE subject_id=$1 AND memory_id=$2",
            "schema" => "DELETE FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2",
            "episode" => "DELETE FROM episode_objects WHERE subject_id=$1 AND episode_id=$2",
            "journal" => "DELETE FROM journal_objects WHERE subject_id=$1 AND journal_id=$2",
            _ => unreachable!("validated cognition kind"),
        };
        sqlx::query(delete)
            .bind(subject.0)
            .bind(id)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation
            .commit("purged", Some(&id.to_string()), None, None)
            .await
    }

    async fn invalidate_cognition_change(
        &self,
        mutation: &mut MutationEnvelope<'_>,
        subject: SubjectId,
        kind: &str,
        id: Uuid,
        reason: &str,
    ) -> Result<()> {
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        if kind == "journal" {
            self.wake_journal_revalidation_in(mutation.tx(), subject, JournalId(id), sequence)
                .await?;
        }
        self.invalidate_object_dependents_in(mutation.tx(), subject, kind, &[id], sequence, reason)
            .await
    }
}

async fn cognition_revisions_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    kind: &str,
    id: Uuid,
) -> Result<(&'static str, Vec<String>)> {
    let (reference_kind, query) = match kind {
        "memory" => (
            "memory_revision",
            "SELECT memory_revision_id::text FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2",
        ),
        "schema" => (
            "cognitive_schema_revision",
            "SELECT r.schema_revision_id::text FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_id=$2",
        ),
        "episode" => (
            "episode_revision",
            "SELECT episode_revision_id::text FROM episode_revisions WHERE subject_id=$1 AND episode_id=$2",
        ),
        "journal" => (
            "journal_revision",
            "SELECT journal_revision_id::text FROM journal_revisions WHERE subject_id=$1 AND journal_id=$2",
        ),
        _ => unreachable!("validated cognition kind"),
    };
    Ok((
        reference_kind,
        sqlx::query_scalar(query)
            .bind(subject.0)
            .bind(id)
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?,
    ))
}

impl MemoryService {
    pub async fn mutate_memory_lifecycle(
        &self,
        subject: SubjectId,
        target: MemoryId,
        operation: OperationId,
        expected: i64,
        action: LifecycleAction,
    ) -> Result<MemoryView> {
        let revision = self
            .transition_cognition(
                subject,
                CognitiveRef::Memory(target),
                operation,
                expected,
                action,
            )
            .await?;
        self.revision(subject, MemoryRevisionId(revision)).await
    }
    pub async fn purge_memory(
        &self,
        subject: SubjectId,
        target: MemoryId,
        operation: OperationId,
        expected: i64,
    ) -> Result<()> {
        self.erase_cognition(subject, CognitiveRef::Memory(target), operation, expected)
            .await
    }
    pub async fn mutate_schema_lifecycle(
        &self,
        subject: SubjectId,
        target: CognitiveSchemaId,
        operation: OperationId,
        expected: i64,
        action: LifecycleAction,
    ) -> Result<SchemaView> {
        let revision = self
            .transition_cognition(
                subject,
                CognitiveRef::CognitiveSchema(target),
                operation,
                expected,
                action,
            )
            .await?;
        self.schema_at(subject, target, Some(CognitiveSchemaRevisionId(revision)))
            .await
    }
    pub async fn purge_schema(
        &self,
        subject: SubjectId,
        target: CognitiveSchemaId,
        operation: OperationId,
        expected: i64,
    ) -> Result<()> {
        self.erase_cognition(
            subject,
            CognitiveRef::CognitiveSchema(target),
            operation,
            expected,
        )
        .await
    }
    pub async fn mutate_episode_lifecycle(
        &self,
        subject: SubjectId,
        target: EpisodeId,
        operation: OperationId,
        expected: i64,
        action: LifecycleAction,
    ) -> Result<EpisodeView> {
        let revision = self
            .transition_cognition(
                subject,
                CognitiveRef::Episode(target),
                operation,
                expected,
                action,
            )
            .await?;
        self.episode(subject, target, Some(EpisodeRevisionId(revision)))
            .await
    }
    pub async fn purge_episode(
        &self,
        subject: SubjectId,
        target: EpisodeId,
        operation: OperationId,
        expected: i64,
    ) -> Result<()> {
        self.erase_cognition(subject, CognitiveRef::Episode(target), operation, expected)
            .await
    }
    pub async fn mutate_journal_lifecycle(
        &self,
        subject: SubjectId,
        target: JournalId,
        operation: OperationId,
        expected: i64,
        action: LifecycleAction,
    ) -> Result<JournalView> {
        let revision = self
            .transition_cognition(
                subject,
                CognitiveRef::Journal(target),
                operation,
                expected,
                action,
            )
            .await?;
        self.journal(subject, target, Some(JournalRevisionId(revision)))
            .await
    }
    pub async fn purge_journal(
        &self,
        subject: SubjectId,
        target: JournalId,
        operation: OperationId,
        expected: i64,
    ) -> Result<()> {
        self.erase_cognition(subject, CognitiveRef::Journal(target), operation, expected)
            .await
    }
}
