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
            _ => return Err(Error::Invalid("invalid Journal lifecycle action".into())),
        };
        let digest = operation_digest(
            "journal_lifecycle",
            subject,
            &serde_json::json!({"journal":journal,"epoch":expected_epoch,"action":action}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation, "journal_lifecycle", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state != "committed" {
                    return Err(Error::Unavailable(
                        "Journal lifecycle is in progress".into(),
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
        .ok_or_else(|| Error::NotFound("Journal not found".into()))?;
        fence_epoch(row.get("object_epoch"), expected_epoch)?;
        require_transition(&row.get::<String, _>(field), from)?;
        if row.get::<String, _>("purge_state") != "normal" {
            return Err(Error::FailedPrecondition(
                "Journal lifecycle transition is invalid".into(),
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

    pub async fn purge_journal(
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
            .start_mutation(subject, operation, "journal_purge", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state != "committed" {
                    return Err(Error::Unavailable("Journal purge is in progress".into()));
                }
                return Ok(());
            }
            MutationStart::Active(mutation) => mutation,
        };
        let epoch:Option<i64>=sqlx::query_scalar("SELECT object_epoch FROM journal_objects WHERE subject_id=$1 AND journal_id=$2 FOR UPDATE")
            .bind(subject.0).bind(journal.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?;
        if epoch != Some(expected_epoch) {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "Journal purge epoch is stale",
            )
            .into());
        }
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
        let mut refs:Vec<String>=sqlx::query_scalar("SELECT journal_revision_id::text FROM journal_revisions WHERE subject_id=$1 AND journal_id=$2")
            .bind(subject.0).bind(journal.0).fetch_all(&mut **mutation.tx()).await.map_err(db)?;
        refs.push(journal.0.to_string());
        self.purge_workflow_content_in(mutation.tx(), subject, &refs)
            .await?;
        sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT e.subject_id,e.consumer_ref,e.event_id,e.request_digest,$3 FROM cognitive_use_events e JOIN journal_revisions r ON r.journal_revision_id::text=e.ref_value WHERE e.subject_id=$1 AND e.ref_kind='journal_revision' AND r.journal_id=$2 ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(journal.0).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM cognitive_use_events e USING journal_revisions r WHERE e.subject_id=$1 AND e.ref_kind='journal_revision' AND e.ref_value=r.journal_revision_id::text AND r.journal_id=$2")
            .bind(subject.0).bind(journal.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM resident_refs rr USING journal_revisions r WHERE rr.ref_kind='journal_revision' AND rr.ref_value=r.journal_revision_id::text AND r.subject_id=$1 AND r.journal_id=$2")
            .bind(subject.0).bind(journal.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM work_context_refs wr USING journal_revisions r WHERE wr.ref_kind='journal_revision' AND wr.ref_value=r.journal_revision_id::text AND r.subject_id=$1 AND r.journal_id=$2")
            .bind(subject.0).bind(journal.0).execute(&mut **mutation.tx()).await.map_err(db)?;

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
