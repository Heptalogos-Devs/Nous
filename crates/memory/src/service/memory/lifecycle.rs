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
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self.memory(subject, memory, None).await;
                }
                return Err(Error::Unavailable(
                    "lifecycle operation is already in progress".into(),
                ));
            }
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
        match self
            .start_mutation(subject, operation_id, "purge_memory", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return Ok(());
                }
            }
            MutationStart::Active(mut mutation) => {
                let row=sqlx::query("SELECT object_epoch,purge_state FROM memory_objects WHERE subject_id=$1 AND memory_id=$2 FOR UPDATE").bind(subject.0).bind(memory.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(||Error::NotFound("memory not found".into()))?;
                let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
                let purge_state: String = row.try_get("purge_state").map_err(db)?;
                fence_epoch(epoch, expected_object_epoch)?;
                if purge_state != "purging" {
                    sqlx::query("UPDATE memory_objects SET purge_state='purging',object_epoch=object_epoch+1 WHERE subject_id=$1 AND memory_id=$2").bind(subject.0).bind(memory.0).execute(&mut **mutation.tx()).await.map_err(db)?;
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
                }
                sqlx::query("DELETE FROM resident_refs WHERE ref_kind='memory_revision' AND ref_value IN (SELECT memory_revision_id::text FROM memory_revisions WHERE memory_id=$1)").bind(memory.0).execute(&mut **mutation.tx()).await.map_err(db)?;
                mutation.checkpoint().await?;
            }
        }
        self.complete_purge(subject, memory, operation_id, &digest)
            .await
    }

    async fn complete_purge(
        &self,
        subject: SubjectId,
        memory: MemoryId,
        operation_id: OperationId,
        digest: &str,
    ) -> Result<()> {
        let mut mutation = match MutationEnvelope::resume(
            &self.store,
            subject,
            operation_id,
            "purge_memory",
            digest,
            Some(OwnerLock("memory-authority")),
        )
        .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let revisions = sqlx::query_scalar::<_, Uuid>(
            "SELECT memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2",
        )
        .bind(subject.0)
        .bind(memory.0)
        .fetch_all(&mut **mutation.tx())
        .await
        .map_err(db)?;
        let mut workflow_refs = revisions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        workflow_refs.push(memory.0.to_string());
        self.purge_workflow_content_in(mutation.tx(), subject, &workflow_refs)
            .await?;
        sqlx::query("DELETE FROM work_context_refs WHERE ref_kind='memory_revision' AND ref_value=ANY($1::text[])")
            .bind(&workflow_refs).execute(&mut **mutation.tx()).await.map_err(db)?;
        for revision in &revisions {
            sqlx::query("UPDATE cognitive_schema_evidence_links SET revoked_at=COALESCE(revoked_at,$3) WHERE basis_kind='memory_revision' AND basis_ref=$1 AND revoked_at IS NULL AND subject_id=$2")
                .bind(revision.to_string()).bind(subject.0).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
            let use_rows=sqlx::query("SELECT subject_id,consumer_ref,event_id,request_digest FROM cognitive_use_events WHERE ref_kind='memory_revision' AND ref_value=$1").bind(revision.to_string()).fetch_all(&mut **mutation.tx()).await.map_err(db)?;
            for row in use_rows {
                sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING").bind(row.try_get::<Uuid,_>("subject_id").map_err(db)?).bind(row.try_get::<String,_>("consumer_ref").map_err(db)?).bind(row.try_get::<Uuid,_>("event_id").map_err(db)?).bind(row.try_get::<String,_>("request_digest").map_err(db)?).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
            }
            sqlx::query("DELETE FROM cognitive_use_events WHERE ref_kind='memory_revision' AND ref_value=$1").bind(revision.to_string()).execute(&mut **mutation.tx()).await.map_err(db)?;
            sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND association_evidence_id IN (SELECT association_evidence_id FROM association_evidence_basis WHERE basis_kind='memory_revision' AND basis_ref=$2)")
                .bind(subject.0).bind(revision.to_string()).execute(&mut **mutation.tx()).await.map_err(db)?;
        }
        sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND ((from_ref_kind='memory_revision' AND from_ref IN (SELECT memory_revision_id::text FROM memory_revisions WHERE memory_id=$2)) OR (to_ref_kind='memory_revision' AND to_ref IN (SELECT memory_revision_id::text FROM memory_revisions WHERE memory_id=$2)))").bind(subject.0).bind(memory.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM memory_objects WHERE subject_id=$1 AND memory_id=$2")
            .bind(subject.0)
            .bind(memory.0)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
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
    ) -> Result<()> {
        AuthorityStore::purge_workflows_in(tx, subject, refs).await?;
        Ok(())
    }
}
