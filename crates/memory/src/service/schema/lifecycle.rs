// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Debug, Clone, Copy)]
pub enum SchemaLifecycleAction {
    Suppress,
    Restore,
    Withdraw,
    Reaccept,
}

impl SchemaLifecycleAction {
    fn transition(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Suppress => ("suppression_state", "normal", "suppressed"),
            Self::Restore => ("suppression_state", "suppressed", "normal"),
            Self::Withdraw => ("acceptance_state", "accepted", "withdrawn"),
            Self::Reaccept => ("acceptance_state", "withdrawn", "accepted"),
        }
    }
}

impl MemoryService {
    pub async fn mutate_schema_lifecycle(
        &self,
        subject: SubjectId,
        schema: CognitiveSchemaId,
        operation: OperationId,
        expected_epoch: i64,
        action: SchemaLifecycleAction,
    ) -> Result<SchemaView> {
        let (field, from, to) = action.transition();
        let digest = operation_digest(
            "schema_lifecycle",
            subject,
            &serde_json::json!({"schema":schema,"epoch":expected_epoch,"field":field,"from":from,"to":to}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation, "schema_lifecycle", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                return self
                    .schema_at(
                        subject,
                        schema,
                        receipt.result_revision.map(CognitiveSchemaRevisionId),
                    )
                    .await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT object_epoch,current_revision_id,acceptance_state,suppression_state FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2 FOR UPDATE")
            .bind(subject.0).bind(schema.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("CognitiveSchema not found".into()))?;
        fence_epoch(row.get("object_epoch"), expected_epoch)?;
        require_transition(&row.get::<String, _>(field), from)?;
        let statement = if field == "suppression_state" {
            "UPDATE cognitive_schemas SET suppression_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND schema_id=$2"
        } else {
            "UPDATE cognitive_schemas SET acceptance_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND schema_id=$2"
        };
        sqlx::query(statement)
            .bind(subject.0)
            .bind(schema.0)
            .bind(to)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        sqlx::query("DELETE FROM resident_refs rr USING cognitive_schema_revisions r WHERE rr.ref_kind='cognitive_schema_revision' AND rr.ref_value=r.schema_revision_id::text AND r.schema_id=$1")
            .bind(schema.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "schema",
            &[schema.0],
            sequence,
            "source_lifecycle_changed",
        )
        .await?;
        let revision: Uuid = row.get("current_revision_id");
        mutation
            .commit(
                "schema",
                Some(&schema.0.to_string()),
                Some(revision),
                Some(expected_epoch + 1),
            )
            .await?;
        self.schema_at(subject, schema, Some(CognitiveSchemaRevisionId(revision)))
            .await
    }

    pub async fn purge_schema(
        &self,
        subject: SubjectId,
        schema: CognitiveSchemaId,
        operation: OperationId,
        expected_epoch: i64,
    ) -> Result<()> {
        let digest = operation_digest(
            "schema_purge",
            subject,
            &serde_json::json!({"schema":schema,"epoch":expected_epoch}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation, "schema_purge", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                let _ = receipt;
                return Ok(());
            }
            MutationStart::Active(mutation) => mutation,
        };
        let epoch: i64 = sqlx::query_scalar("SELECT object_epoch FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2 FOR UPDATE")
            .bind(subject.0).bind(schema.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("CognitiveSchema not found".into()))?;
        fence_epoch(epoch, expected_epoch)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "schema",
            &[schema.0],
            sequence,
            "basis_purged",
        )
        .await?;
        let revisions: Vec<String> = sqlx::query_scalar(
            "SELECT schema_revision_id::text FROM cognitive_schema_revisions WHERE schema_id=$1",
        )
        .bind(schema.0)
        .fetch_all(&mut **mutation.tx())
        .await
        .map_err(db)?;
        let mut refs = revisions.clone();
        refs.push(schema.0.to_string());
        self.purge_workflow_content_in(mutation.tx(), subject, &refs)
            .await?;
        sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT subject_id,consumer_ref,event_id,request_digest,$3 FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='cognitive_schema_revision' AND ref_value=ANY($2::text[]) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(&revisions).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='cognitive_schema_revision' AND ref_value=ANY($2::text[])")
            .bind(subject.0).bind(&revisions).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM resident_refs WHERE ref_kind='cognitive_schema_revision' AND ref_value=ANY($1::text[])")
            .bind(&revisions).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM work_context_refs WHERE ref_kind='cognitive_schema_revision' AND ref_value=ANY($1::text[])")
            .bind(&revisions).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("UPDATE cognitive_schema_evidence_links SET revoked_at=COALESCE(revoked_at,$3) WHERE subject_id=$1 AND basis_kind='cognitive_schema_revision' AND basis_ref=ANY($2::text[])")
            .bind(subject.0).bind(&revisions).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND ((from_ref_kind='cognitive_schema_revision' AND from_ref=ANY($2::text[])) OR (to_ref_kind='cognitive_schema_revision' AND to_ref=ANY($2::text[])) OR association_evidence_id IN (SELECT association_evidence_id FROM association_evidence_basis WHERE basis_kind='cognitive_schema_revision' AND basis_ref=ANY($2::text[])))")
            .bind(subject.0).bind(&revisions).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query("DELETE FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2")
            .bind(subject.0)
            .bind(schema.0)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation
            .commit("purged", Some(&schema.0.to_string()), None, None)
            .await
    }
}
