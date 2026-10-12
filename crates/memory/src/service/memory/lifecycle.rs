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
            .start_mutation(subject, operation_id, field, &digest)
            .await?
        {
            MutationStart::Replay(_) => return self.memory(subject, memory, None).await,
            MutationStart::Active(mutation) => mutation,
        };
        let row = match field {
            "suppression_state" => sqlx::query("SELECT object_epoch,suppression_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE"),
            "acceptance_state" => sqlx::query("SELECT object_epoch,acceptance_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE"),
            _ => return Err(Error::Internal("unsupported lifecycle state".into())),
        }.bind(subject.0).bind(memory.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(||Error::NotFound("memory not found".into()))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        let current: String = row.try_get(field).map_err(db)?;
        fence_epoch(epoch, expected_object_epoch)?;
        if current == to {
            mutation
                .commit("memory", Some(&memory.0.to_string()), None, Some(epoch))
                .await?;
            return self.memory(subject, memory, None).await;
        }
        require_transition(&current, from)?;
        match field {
            "suppression_state" => sqlx::query("UPDATE memory_objects SET suppression_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2"),
            "acceptance_state" => sqlx::query("UPDATE memory_objects SET acceptance_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2"),
            _ => return Err(Error::Internal("unsupported lifecycle state".into())),
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

    pub async fn purge_memory(
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
        let mut mutation = match self
            .start_mutation(subject, operation_id, "purge_memory", &digest)
            .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let epoch: Option<i64> = sqlx::query_scalar(
                    "SELECT object_epoch FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE",
                )
                .bind(subject.0)
                .bind(memory.0)
                .fetch_optional(&mut **mutation.tx())
                .await
                .map_err(db)?;
        let epoch = epoch.ok_or_else(|| Error::NotFound("memory not found".into()))?;
        fence_epoch(epoch, expected_object_epoch)?;

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

        let revisions: Vec<String> = sqlx::query_scalar(
                    "SELECT memory_revision_id::text FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2",
                )
                .bind(subject.0)
                .bind(memory.0)
                .fetch_all(&mut **mutation.tx())
                .await
                .map_err(db)?;
        let mut workflow_refs = revisions.clone();
        workflow_refs.push(memory.0.to_string());
        self.purge_workflow_content_in(mutation.tx(), subject, &workflow_refs)
            .await?;

        sqlx::query("DELETE FROM resident_refs WHERE ref_kind='memory_revision' AND ref_value=ANY($1::text[])")
                    .bind(&revisions)
                    .execute(&mut **mutation.tx())
                    .await
                    .map_err(db)?;
        sqlx::query("DELETE FROM work_context_refs WHERE ref_kind='memory_revision' AND ref_value=ANY($1::text[])")
                    .bind(&workflow_refs)
                    .execute(&mut **mutation.tx())
                    .await
                    .map_err(db)?;
        sqlx::query("UPDATE cognitive_schema_evidence_links SET revoked_at=COALESCE(revoked_at,$3) WHERE subject_id=$1 AND basis_kind='memory_revision' AND basis_ref=ANY($2::text[])")
                    .bind(subject.0)
                    .bind(&revisions)
                    .bind(self.cognition.now(subject))
                    .execute(&mut **mutation.tx())
                    .await
                    .map_err(db)?;
        sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT subject_id,consumer_ref,event_id,request_digest,$3 FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='memory_revision' AND ref_value=ANY($2::text[]) ON CONFLICT DO NOTHING")
                    .bind(subject.0)
                    .bind(&revisions)
                    .bind(self.cognition.now(subject))
                    .execute(&mut **mutation.tx())
                    .await
                    .map_err(db)?;
        sqlx::query("DELETE FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='memory_revision' AND ref_value=ANY($2::text[])")
                    .bind(subject.0)
                    .bind(&revisions)
                    .execute(&mut **mutation.tx())
                    .await
                    .map_err(db)?;
        sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND ((from_ref_kind='memory_revision' AND from_ref=ANY($2::text[])) OR (to_ref_kind='memory_revision' AND to_ref=ANY($2::text[])) OR association_evidence_id IN (SELECT association_evidence_id FROM association_evidence_basis WHERE basis_kind='memory_revision' AND basis_ref=ANY($2::text[])))")
                    .bind(subject.0)
                    .bind(&revisions)
                    .execute(&mut **mutation.tx())
                    .await
                    .map_err(db)?;
        sqlx::query("DELETE FROM memory_objects WHERE subject_id=$1 AND memory_id=$2")
            .bind(subject.0)
            .bind(memory.0)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation
            .commit("purged", Some(&memory.0.to_string()), None, None)
            .await?;
        Ok(())
    }
}

impl MemoryService {
    pub(crate) async fn purge_workflow_content_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        refs: &[String],
    ) -> Result<()> {
        AuthorityStore::purge_workflows_in(tx, subject, refs).await?;
        Ok(())
    }
}
