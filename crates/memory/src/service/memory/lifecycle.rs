// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;

impl MemoryService {
    pub async fn suppress(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
    ) -> Result<MemoryView> {
        self.mutate_lifecycle(
            subject,
            memory,
            operation_id,
            expected_object_epoch,
            "suppression_state",
            "normal",
            "suppressed",
        )
        .await
    }

    pub async fn restore(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
    ) -> Result<MemoryView> {
        self.mutate_lifecycle(
            subject,
            memory,
            operation_id,
            expected_object_epoch,
            "suppression_state",
            "suppressed",
            "normal",
        )
        .await
    }

    pub async fn withdraw(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
    ) -> Result<MemoryView> {
        self.mutate_lifecycle(
            subject,
            memory,
            operation_id,
            expected_object_epoch,
            "acceptance_state",
            "accepted",
            "withdrawn",
        )
        .await
    }

    pub async fn reaccept(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
    ) -> Result<MemoryView> {
        self.mutate_lifecycle(
            subject,
            memory,
            operation_id,
            expected_object_epoch,
            "acceptance_state",
            "withdrawn",
            "accepted",
        )
        .await
    }

    async fn mutate_lifecycle(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
        field: &str,
        from: &str,
        to: &str,
    ) -> Result<MemoryView> {
        let digest = operation_digest(
            field,
            subject,
            &serde_json::json!({"memory_id":memory,"expected_object_epoch":expected_object_epoch,"from":from,"to":to}),
        )?;
        let mut mutation = match self
            .start_mutation(ErrorOwner::Memory, subject, operation_id, field, &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self.memory(subject, memory, None).await;
                }
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::Unavailable,
                    "lifecycle operation is already in progress",
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row = match field {
            "suppression_state" => sqlx::query("SELECT object_epoch,suppression_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE"),
            "acceptance_state" => sqlx::query("SELECT object_epoch,acceptance_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE"),
            _ => return Err(Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InternalFailure, "unsupported lifecycle state")),
        }.bind(subject.0).bind(memory.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(||Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::NotFound, "memory not found"))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        let current: String = row.try_get(field).map_err(db)?;
        fence_epoch(ErrorOwner::Memory, epoch, expected_object_epoch)?;
        if current == to {
            mutation
                .commit("memory", Some(&memory.0.to_string()), None, Some(epoch))
                .await?;
            return self.memory(subject, memory, None).await;
        }
        require_transition(ErrorOwner::Memory, &current, from)?;
        match field {
            "suppression_state" => sqlx::query("UPDATE memory_objects SET suppression_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2"),
            "acceptance_state" => sqlx::query("UPDATE memory_objects SET acceptance_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2"),
            _ => return Err(Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InternalFailure, "unsupported lifecycle state")),
        }.bind(subject.0).bind(memory.0).bind(to).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM resident_refs WHERE ref_kind='memory_revision' AND ref_value IN (SELECT memory_revision_id::text FROM memory_revisions WHERE memory_id=$1)").bind(memory.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "memory",
            &[memory.0],
            sequence,
            "source_lifecycle_changed",
        )
        .await?;
        mutation
            .commit("memory", Some(&memory.0.to_string()), None, Some(epoch + 1))
            .await?;
        self.memory(subject, memory, None).await
    }

    /// Persist the owner-selected purge checkpoint. Kernel coordinates Serving
    /// reclamation before calling `complete_purge_memory`.
    pub async fn begin_purge_memory(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
    ) -> Result<()> {
        let digest = operation_digest(
            "purge_memory",
            subject,
            &serde_json::json!({"memory_id":memory,"expected_object_epoch":expected_object_epoch}),
        )?;
        match self
            .resume_mutation(
                ErrorOwner::Memory,
                subject,
                operation_id,
                "purge_memory",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return Ok(());
                }
            }
            MutationStart::Active(mut mutation) => {
                let row=sqlx::query("SELECT object_epoch,purge_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE").bind(subject.0).bind(memory.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(||Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::NotFound, "memory not found"))?;
                let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
                let purge_state: String = row.try_get("purge_state").map_err(db)?;
                if purge_state == "purging" {
                    if epoch == expected_object_epoch + 1
                        && mutation
                            .checkpoint_matches("purging", "memory", &memory.0.to_string(), epoch)
                            .await?
                    {
                        return mutation.checkpoint().await;
                    }
                    return Err(Error::new(
                        ErrorOwner::Memory,
                        ErrorCode::OperationInProgress,
                        "Memory purge is already in progress",
                    ));
                }
                fence_epoch(ErrorOwner::Memory, epoch, expected_object_epoch)?;
                sqlx::query("UPDATE memory_objects SET purge_state='purging',object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2")
                    .bind(subject.0).bind(memory.0).execute(&mut **mutation.tx()).await.map_err(db)?;
                let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
                self.invalidate_object_dependents_in(
                    mutation.tx(),
                    subject,
                    "memory",
                    &[memory.0],
                    sequence,
                    "basis_purged",
                )
                .await?;
                AuthorityStore::advance_serving_purge_epoch_in(mutation.tx(), subject).await?;
                let revisions = sqlx::query_scalar::<_, Uuid>(
                    "SELECT memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2",
                )
                .bind(subject.0)
                .bind(memory.0)
                .fetch_all(&mut **mutation.tx())
                .await
                .map_err(db)?;
                let mut identity_refs = vec![CognitiveRef::Memory(memory)];
                identity_refs.extend(
                    revisions
                        .iter()
                        .copied()
                        .map(|id| CognitiveRef::MemoryRevision(MemoryRevisionId(id))),
                );
                AuthorityStore::tombstone_identity_refs_in(mutation.tx(), subject, &identity_refs)
                    .await?;
                let revision_refs = revisions
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                let mut workflow_refs = revision_refs.clone();
                workflow_refs.push(memory.0.to_string());
                self.purge_workflow_content_in(
                    mutation.tx(),
                    subject,
                    &workflow_refs,
                    &identity_refs,
                )
                .await?;
                sqlx::query("DELETE FROM resident_refs WHERE ref_kind='memory_revision' AND ref_value IN (SELECT memory_revision_id::text FROM memory_revisions WHERE memory_id=$1)").bind(memory.0).execute(&mut **mutation.tx()).await.map_err(db)?;
                AuthorityStore::purge_embedding_material_uses_in(
                    mutation.tx(),
                    subject,
                    &identity_refs,
                )
                .await?;
                AuthorityStore::purge_owner_history_in(mutation.tx(), subject, &identity_refs)
                    .await?;
                sqlx::query("UPDATE cognitive_schema_evidence_links SET revoked_at=COALESCE(revoked_at,$3) WHERE subject_id=$1 AND basis_kind='memory_revision' AND basis_ref=ANY($2::text[]) AND revoked_at IS NULL")
                    .bind(subject.0).bind(&revision_refs).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
                sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT subject_id,consumer_ref,event_id,request_digest,$3 FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='memory_revision' AND ref_value=ANY($2::text[]) ON CONFLICT DO NOTHING")
                    .bind(subject.0).bind(&revision_refs).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
                sqlx::query("DELETE FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='memory_revision' AND ref_value=ANY($2::text[])")
                    .bind(subject.0).bind(&revision_refs).execute(&mut **mutation.tx()).await.map_err(db)?;
                sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND association_evidence_id IN (SELECT association_evidence_id FROM association_evidence_basis WHERE basis_kind='memory_revision' AND basis_ref=ANY($2::text[]))")
                    .bind(subject.0).bind(&revision_refs).execute(&mut **mutation.tx()).await.map_err(db)?;
                sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND ((from_ref_kind='memory_revision' AND from_ref=ANY($2::text[])) OR (to_ref_kind='memory_revision' AND to_ref=ANY($2::text[])))")
                    .bind(subject.0).bind(&revision_refs).execute(&mut **mutation.tx()).await.map_err(db)?;
                mutation
                    .checkpoint_with_result(
                        "purging",
                        "memory",
                        &memory.0.to_string(),
                        expected_object_epoch + 1,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Final owner cleanup after Kernel confirms all prior-epoch Serving
    /// generations have no live readers.
    pub async fn complete_purge_memory(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        expected_object_epoch: i64,
    ) -> Result<()> {
        let digest = operation_digest(
            "purge_memory",
            subject,
            &serde_json::json!({"memory_id":memory,"expected_object_epoch":expected_object_epoch}),
        )?;
        let mut mutation = match MutationEnvelope::resume(
            &self.store,
            subject,
            operation_id,
            "purge_memory",
            &digest,
            Some(OwnerLock("memory-authority")),
            ErrorOwner::Memory,
        )
        .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query(
            "SELECT object_epoch,purge_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE",
        )
        .bind(subject.0)
        .bind(memory.0)
        .fetch_optional(&mut **mutation.tx())
        .await
        .map_err(db)?
        .ok_or_else(|| Error::new(ErrorOwner::Memory, ErrorCode::NotFound, "memory not found"))?;
        if row.try_get::<String, _>("purge_state").map_err(db)? != "purging"
            || row.try_get::<i64, _>("object_epoch").map_err(db)? != expected_object_epoch + 1
        {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::FailedPrecondition,
                "Memory purge checkpoint is missing or changed",
            ));
        }
        if !mutation
            .checkpoint_matches(
                "purging",
                "memory",
                &memory.0.to_string(),
                expected_object_epoch + 1,
            )
            .await?
        {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::OperationInProgress,
                "Memory purge checkpoint belongs to another operation",
            ));
        }
        sqlx::query("SELECT subject_id FROM subjects WHERE subject_id=$1 FOR UPDATE")
            .bind(subject.0)
            .fetch_one(&mut **mutation.tx())
            .await
            .map_err(db)?;
        if AuthorityStore::has_unreclaimed_serving_artifact_in(mutation.tx(), subject).await? {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::OperationInProgress,
                "Serving purge reclamation remains in progress",
            ));
        }
        sqlx::query("DELETE FROM memory_objects WHERE subject_id=$1 AND memory_id=$2")
            .bind(subject.0)
            .bind(memory.0)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation
            .commit("purged", Some(&memory.0.to_string()), None, None)
            .await
    }
}

impl MemoryService {
    pub(crate) async fn purge_workflow_content_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        refs: &[String],
        identity_refs: &[CognitiveRef],
    ) -> Result<()> {
        AuthorityStore::purge_workflows_in(tx, subject, refs).await?;
        AuthorityStore::purge_work_contexts_in(tx, subject, identity_refs).await?;
        Ok(())
    }
}
