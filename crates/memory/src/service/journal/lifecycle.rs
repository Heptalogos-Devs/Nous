// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn mutate_journal_lifecycle(
        &self,
        subject: SubjectId,
        journal: JournalId,
        operation: OperationId,
        expected_epoch: i64,
        action: &str,
    ) -> Result<JournalView> {
        let (field, from, to) = match action {
            "suppress" => ("suppression_state", "normal", "suppressed"),
            "restore" => ("suppression_state", "suppressed", "normal"),
            "withdraw" => ("acceptance_state", "accepted", "withdrawn"),
            "reaccept" => ("acceptance_state", "withdrawn", "accepted"),
            _ => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "invalid Journal lifecycle action",
                ));
            }
        };
        let digest = operation_digest(
            "journal_lifecycle",
            subject,
            &serde_json::json!({"journal":journal,"epoch":expected_epoch,"action":action}),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Memory,
                subject,
                operation,
                "journal_lifecycle",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state != "committed" {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::Unavailable,
                        "Journal lifecycle is in progress",
                    ));
                }
                return self
                    .journal(
                        subject,
                        journal,
                        receipt.result_revision.map(JournalRevisionId),
                    )
                    .await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        sqlx::query("SELECT subject_id FROM subjects WHERE subject_id=$1 FOR UPDATE")
            .bind(subject.0)
            .fetch_one(&mut **mutation.tx())
            .await
            .map_err(db)?;
        let row = sqlx::query(
            "SELECT * FROM journal_objects WHERE subject_id=$1 AND journal_id=$2 FOR UPDATE",
        )
        .bind(subject.0)
        .bind(journal.0)
        .fetch_optional(&mut **mutation.tx())
        .await
        .map_err(db)?
        .ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::NotFound,
                "Journal not found",
            )
        })?;
        fence_epoch(ErrorOwner::Memory, row.get("object_epoch"), expected_epoch)?;
        require_transition(ErrorOwner::Memory, &row.get::<String, _>(field), from)?;
        if row.get::<String, _>("purge_state") != "normal" {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::FailedPrecondition,
                "Journal lifecycle transition is invalid",
            ));
        }
        let statement = if field == "suppression_state" {
            "UPDATE journal_objects SET suppression_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND journal_id=$2"
        } else {
            "UPDATE journal_objects SET acceptance_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND journal_id=$2"
        };
        sqlx::query(statement)
            .bind(subject.0)
            .bind(journal.0)
            .bind(to)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::text()).await?;
        self.wake_journal_revalidation_in(mutation.tx(), subject, journal, sequence)
            .await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "journal",
            &[journal.0],
            sequence,
            "source_lifecycle_changed",
        )
        .await?;
        let revision: Uuid = row.get("current_revision_id");

        mutation
            .commit(
                "journal",
                Some(&journal.0.to_string()),
                Some(revision),
                None,
            )
            .await?;
        self.journal(subject, journal, Some(JournalRevisionId(revision)))
            .await
    }

    pub async fn begin_purge_journal(
        &self,
        subject: SubjectId,
        journal: JournalId,
        operation: OperationId,
        expected_epoch: i64,
    ) -> Result<()> {
        let digest = operation_digest(
            "journal_purge",
            subject,
            &serde_json::json!({"journal":journal,"epoch":expected_epoch}),
        )?;
        let mut mutation = match self
            .resume_mutation(
                ErrorOwner::Memory,
                subject,
                operation,
                "journal_purge",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT object_epoch,purge_state FROM journal_objects WHERE subject_id=$1 AND journal_id=$2 FOR UPDATE")
            .bind(subject.0).bind(journal.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?
            .ok_or_else(|| Error::new(ErrorOwner::Memory, ErrorCode::NotFound, "Journal not found"))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        if row.try_get::<String, _>("purge_state").map_err(db)? == "purging" {
            if epoch == expected_epoch + 1
                && mutation
                    .checkpoint_matches("purging", "journal", &journal.0.to_string(), epoch)
                    .await?
            {
                return mutation.checkpoint().await;
            }
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::OperationInProgress,
                "Journal purge is already in progress",
            ));
        }
        crate::service::state::fence_epoch(ErrorOwner::Memory, epoch, expected_epoch)?;
        let revision_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT journal_revision_id FROM journal_revisions WHERE subject_id=$1 AND journal_id=$2",
        )
        .bind(subject.0)
        .bind(journal.0)
        .fetch_all(&mut **mutation.tx())
        .await
        .map_err(db)?;
        let mut identity_refs = vec![CognitiveRef::Journal(journal)];
        identity_refs.extend(
            revision_ids
                .iter()
                .copied()
                .map(|id| CognitiveRef::JournalRevision(JournalRevisionId(id))),
        );
        sqlx::query("UPDATE journal_objects SET purge_state='purging',object_epoch=object_epoch+1 WHERE subject_id=$1 AND journal_id=$2")
            .bind(subject.0).bind(journal.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::text()).await?;
        self.wake_journal_revalidation_in(mutation.tx(), subject, journal, sequence)
            .await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "journal",
            &[journal.0],
            sequence,
            "source_purged",
        )
        .await?;
        AuthorityStore::advance_serving_purge_epoch_in(mutation.tx(), subject).await?;
        AuthorityStore::tombstone_identity_refs_in(mutation.tx(), subject, &identity_refs).await?;
        let revision_refs = revision_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let mut workflow_refs = revision_refs.clone();
        workflow_refs.push(journal.0.to_string());
        self.purge_workflow_content_in(mutation.tx(), subject, &workflow_refs, &identity_refs)
            .await?;
        sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT e.subject_id,e.consumer_ref,e.event_id,e.request_digest,$3 FROM cognitive_use_events e JOIN journal_revisions r ON r.journal_revision_id::text=e.ref_value WHERE e.subject_id=$1 AND e.ref_kind='journal_revision' AND r.journal_id=$2 ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(journal.0).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM cognitive_use_events e USING journal_revisions r WHERE e.subject_id=$1 AND e.ref_kind='journal_revision' AND e.ref_value=r.journal_revision_id::text AND r.journal_id=$2")
            .bind(subject.0).bind(journal.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM resident_refs rr USING journal_revisions r WHERE rr.ref_kind='journal_revision' AND rr.ref_value=r.journal_revision_id::text AND r.subject_id=$1 AND r.journal_id=$2")
            .bind(subject.0).bind(journal.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("UPDATE cognitive_schema_evidence_links SET revoked_at=COALESCE(revoked_at,$3) WHERE subject_id=$1 AND basis_kind='journal_revision' AND basis_ref=ANY($2::text[])")
            .bind(subject.0).bind(&revision_refs).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND ((from_ref_kind='journal_revision' AND from_ref=ANY($2::text[])) OR (to_ref_kind='journal_revision' AND to_ref=ANY($2::text[])) OR association_evidence_id IN (SELECT association_evidence_id FROM association_evidence_basis WHERE basis_kind='journal_revision' AND basis_ref=ANY($2::text[])))")
            .bind(subject.0).bind(&revision_refs).execute(&mut **mutation.tx()).await.map_err(db)?;
        AuthorityStore::purge_embedding_material_uses_in(mutation.tx(), subject, &identity_refs)
            .await?;
        AuthorityStore::purge_owner_history_in(mutation.tx(), subject, &identity_refs).await?;
        mutation
            .checkpoint_with_result(
                "purging",
                "journal",
                &journal.0.to_string(),
                expected_epoch + 1,
            )
            .await
    }

    pub async fn complete_purge_journal(
        &self,
        subject: SubjectId,
        journal: JournalId,
        operation: OperationId,
        expected_epoch: i64,
    ) -> Result<()> {
        let digest = operation_digest(
            "journal_purge",
            subject,
            &serde_json::json!({"journal":journal,"epoch":expected_epoch}),
        )?;
        let mut mutation = match MutationEnvelope::resume(
            &self.store,
            subject,
            operation,
            "journal_purge",
            &digest,
            Some(OwnerLock("memory-authority")),
            ErrorOwner::Memory,
        )
        .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT object_epoch,purge_state FROM journal_objects WHERE subject_id=$1 AND journal_id=$2 FOR UPDATE")
            .bind(subject.0).bind(journal.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?
            .ok_or_else(|| Error::new(ErrorOwner::Memory, ErrorCode::NotFound, "Journal not found"))?;
        if row.try_get::<String, _>("purge_state").map_err(db)? != "purging"
            || row.try_get::<i64, _>("object_epoch").map_err(db)? != expected_epoch + 1
        {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::FailedPrecondition,
                "Journal purge checkpoint is missing or changed",
            ));
        }
        if !mutation
            .checkpoint_matches(
                "purging",
                "journal",
                &journal.0.to_string(),
                expected_epoch + 1,
            )
            .await?
        {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::OperationInProgress,
                "Journal purge checkpoint belongs to another operation",
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
        sqlx::query("DELETE FROM journal_objects WHERE subject_id=$1 AND journal_id=$2")
            .bind(subject.0)
            .bind(journal.0)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation
            .commit("purged", Some(&journal.0.to_string()), None, None)
            .await
    }
}
