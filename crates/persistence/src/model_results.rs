// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use nous_core::{DomainError, DomainErrorCode, Error, Result, SubjectId};
use nous_core::{ExecutionTelemetry, ModelResultContext, ResultPayload};
use sqlx::Row;
use sqlx::{Postgres, Transaction, types::Json};
use uuid::Uuid;

/// Semantic owner identity; storage does not enumerate cognition domains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResultOwner(String);
impl ModelResultOwner {
    pub fn new(value: &str) -> Result<Self> {
        if !(1..=64).contains(&value.len())
            || !value.as_bytes()[0].is_ascii_lowercase()
            || !value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err(Error::Invalid("invalid workflow owner identifier".into()));
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A 1 MiB Material string can occupy 6 MiB after JSON escaping, plus metadata.
pub const MODEL_RESULT_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ModelOperationIdentity {
    pub subject: SubjectId,
    pub owner: String,
    pub operation_key: String,
    pub semantic_digest: String,
}

pub struct ModelResultRecord {
    pub context: Option<ModelResultContext>,
    pub proposal: Option<ResultPayload>,
    pub outcome: Option<ResultPayload>,
    pub execution_telemetry: Option<ExecutionTelemetry>,
}

impl ModelResultRecord {
    fn from_row(row: &sqlx::postgres::PgRow, owner: &str, key: &str, digest: &str) -> Result<Self> {
        if row.try_get::<String, _>("semantic_digest").map_err(db)? != digest {
            return Err(DomainError::new(
                DomainErrorCode::OperationIdConflict,
                "Model operation identity has different semantic input",
            )
            .with_context("owner", owner)
            .with_context("operation_key", key)
            .into());
        }
        Ok(ModelResultRecord {
            context: row
                .try_get::<Option<Json<ModelResultContext>>, _>("context")
                .map_err(db)?
                .map(|value| value.0),
            proposal: row
                .try_get::<Option<Json<ResultPayload>>, _>("proposal")
                .map_err(db)?
                .map(|v| v.0),
            outcome: row
                .try_get::<Option<Json<ResultPayload>>, _>("outcome")
                .map_err(db)?
                .map(|v| v.0),
            execution_telemetry: row
                .try_get::<Option<Json<ExecutionTelemetry>>, _>("execution_telemetry")
                .map_err(db)?
                .map(|v| v.0),
        })
    }
}

impl AuthorityStore {
    pub async fn add_model_result_dependencies_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        owner: &str,
        operation: nous_core::OperationId,
        dependencies: &[nous_core::CognitiveRef],
    ) -> Result<()> {
        let rows = sqlx::query("SELECT outcome FROM model_results WHERE subject_id=$1 AND (owner=$2 OR owner='subject_reflection') AND (operation_key=$3::uuid::text OR $3=ANY(mutation_operations)) FOR UPDATE")
            .bind(subject.0).bind(owner).bind(operation.0).fetch_all(&mut **tx).await.map_err(db)?;
        for row in rows {
            let outcome = row
                .try_get::<Option<Json<ResultPayload>>, _>("outcome")
                .map_err(db)?;
            if outcome.is_some_and(|value| value.0.purged) {
                return Err(Error::NotFound("operation material was purged".into()));
            }
        }
        sqlx::query("UPDATE model_results w SET dependencies=(SELECT COALESCE(jsonb_agg(DISTINCT ref),'[]') FROM jsonb_array_elements(w.dependencies||$4::jsonb) ref) WHERE w.subject_id=$1 AND (w.owner=$2 OR w.owner='subject_reflection') AND (w.operation_key=$3::uuid::text OR $3=ANY(w.mutation_operations)) AND w.outcome IS NULL")
            .bind(subject.0).bind(owner).bind(operation.0).bind(Json(dependencies))
            .execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn publish_material_result_in(
        tx: &mut Transaction<'_, Postgres>,
        identity: &ModelOperationIdentity,
        reference: nous_core::CognitiveRef,
    ) -> Result<()> {
        let updated = sqlx::query("UPDATE model_results SET dependencies=(SELECT COALESCE(jsonb_agg(DISTINCT ref),'[]') FROM jsonb_array_elements(dependencies||$5::jsonb) ref) WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 AND semantic_digest=$4 AND outcome IS NULL")
            .bind(identity.subject.0).bind(&identity.owner).bind(&identity.operation_key)
            .bind(&identity.semantic_digest).bind(Json(vec![reference]))
            .execute(&mut **tx).await.map_err(db)?;
        if updated.rows_affected() != 1 {
            return Err(DomainError::new(
                DomainErrorCode::OperationIdConflict,
                "Material operation is missing, finalized, purged or has different input",
            )
            .into());
        }
        Ok(())
    }

    pub async fn model_result_cognitive_stamp_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        owner: &str,
        key: &str,
    ) -> Result<Option<nous_core::DomainStamp>> {
        let value: Option<Option<Json<ModelResultContext>>> = sqlx::query_scalar("SELECT context FROM model_results WHERE subject_id=$1 AND (owner=$2 OR owner='subject_reflection') AND (operation_key=$3 OR $3::uuid=ANY(mutation_operations))")
            .bind(subject.0).bind(owner).bind(key).fetch_optional(&mut **tx).await.map_err(db)?;
        Ok(value
            .flatten()
            .and_then(|context| context.0.cognitive_formed_at))
    }

    pub async fn read_model_operation(
        &self,
        subject: SubjectId,
        owner: &str,
        key: &str,
        digest: &str,
    ) -> Result<Option<ModelResultRecord>> {
        ModelResultOwner::new(owner)?;
        let row = sqlx::query("SELECT semantic_digest,context,proposal,outcome,execution_telemetry FROM model_results WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
            .bind(subject.0).bind(owner).bind(key).fetch_optional(self.pool()).await.map_err(db)?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(ModelResultRecord::from_row(&row, owner, key, digest)?))
    }

    pub async fn purge_model_results_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        references: &[String],
    ) -> Result<()> {
        sqlx::query("DELETE FROM motivation_evaluations e WHERE subject_id=$1 AND EXISTS(SELECT 1 FROM jsonb_array_elements(e.document->'inputs') dependency WHERE dependency->>'id'=ANY($2::text[]))").bind(subject.0).bind(references).execute(&mut **tx).await.map_err(db)?;
        sqlx::query("UPDATE model_results w SET context=NULL,proposal=NULL,outcome=$3,dependencies='[]',updated_at=clock_timestamp() WHERE subject_id=$1 AND EXISTS(SELECT 1 FROM jsonb_array_elements(w.dependencies) dependency WHERE dependency->>'id'=ANY($2::text[]))")
            .bind(subject.0).bind(references)
            .bind(Json(ResultPayload { purged: true, ..Default::default() }))
            .execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn save_model_operation(
        &self,
        identity: &ModelOperationIdentity,
        context: Option<&ModelResultContext>,
        proposal: Option<&ResultPayload>,
        outcome: Option<&ResultPayload>,
        execution_telemetry: Option<&ExecutionTelemetry>,
        mutation_operations: &[nous_core::OperationId],
    ) -> Result<()> {
        let ModelOperationIdentity {
            subject,
            owner,
            operation_key: key,
            semantic_digest: digest,
        } = identity;
        ModelResultOwner::new(owner)?;
        if key.is_empty()
            || key.len() > 256
            || digest.is_empty()
            || digest.len() > 128
            || (proposal.is_none() && outcome.is_none() && execution_telemetry.is_none())
        {
            return Err(Error::Invalid(
                "model record requires an actual result or execution telemetry".into(),
            ));
        }
        if let Some(context) = context
            && (context.dependencies.len() > 2048
                || serde_json::to_vec(context)
                    .map_err(|error| Error::Invalid(error.to_string()))?
                    .len()
                    > 262144)
        {
            return Err(Error::Invalid("model result context exceeds bound".into()));
        }

        for value in [proposal, outcome].into_iter().flatten() {
            if serde_json::to_vec(value)
                .map_err(|error| Error::Invalid(error.to_string()))?
                .len()
                > MODEL_RESULT_MAX_BYTES
            {
                return Err(Error::Invalid("model workflow value exceeds bound".into()));
            }
        }
        if let Some(telemetry) = execution_telemetry {
            telemetry.validate()?;
        }
        let mut tx = self.begin().await?;
        let maintenance = context.and_then(|value| value.maintenance_trigger.as_ref());
        let revision = maintenance
            .map(|claim| i64::try_from(claim.trigger_revision))
            .transpose()
            .map_err(|_| Error::Invalid("maintenance revision exceeds storage range".into()))?;
        if let Some(claim) = maintenance
            && proposal.is_some()
        {
            let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM maintenance_needs WHERE subject_id=$1 AND need_id=$2 AND state='pending' AND ((trigger_authority_seq=$3 AND trigger_revision=$4) OR (kind='subject_reflection' AND trigger_authority_seq>=$3 AND trigger_revision>=$4)))")
                .bind(subject.0).bind(claim.need_id).bind(claim.trigger_authority_seq).bind(revision)
                .fetch_one(&mut *tx).await.map_err(db)?;
            if !current {
                return Err(DomainError::new(
                    DomainErrorCode::StaleRevision,
                    "maintenance trigger changed",
                )
                .into());
            }
        }
        sqlx::query("INSERT INTO model_results(subject_id,owner,operation_key,semantic_digest,context,maintenance_need_id,maintenance_trigger_revision,dependencies) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(owner).bind(key).bind(digest).bind(context.map(Json))
            .bind(maintenance.map(|claim| claim.need_id)).bind(revision)
            .bind(Json(context.map(|value| value.dependencies.clone()).unwrap_or_default()))
            .execute(&mut *tx).await.map_err(db)?;

        let row = sqlx::query("SELECT dependencies,mutation_operations,execution_telemetry FROM model_results WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 AND semantic_digest=$4 AND outcome IS NULL FOR UPDATE")
            .bind(subject.0).bind(owner).bind(key).bind(digest).fetch_optional(&mut *tx).await.map_err(db)?
            .ok_or_else(|| DomainError::new(DomainErrorCode::OperationIdConflict, "Operation is missing, finalized, purged or has different input")
                .with_context("owner", owner).with_context("operation_key", key))?;
        let mut operations: Vec<Uuid> = row.try_get("mutation_operations").map_err(db)?;
        for operation in mutation_operations {
            if !operations.contains(&operation.0) {
                operations.push(operation.0);
            }
        }
        if operations.len() > 2048 {
            return Err(Error::Invalid("workflow mutation bound exceeded".into()));
        }
        let mut telemetry = row
            .try_get::<Option<Json<ExecutionTelemetry>>, _>("execution_telemetry")
            .map_err(db)?
            .map(|v| v.0);
        if let Some(next) = execution_telemetry {
            telemetry
                .get_or_insert_with(Default::default)
                .append(next.clone());
        }
        let mut dependencies = row
            .try_get::<Json<Vec<nous_core::CognitiveRef>>, _>("dependencies")
            .map_err(db)?
            .0;
        for reference in context
            .into_iter()
            .flat_map(|value| &value.dependencies)
            .chain(
                [proposal, outcome]
                    .into_iter()
                    .flatten()
                    .flat_map(|value| &value.dependencies),
            )
        {
            if !dependencies.contains(reference) {
                dependencies.push(reference.clone());
            }
        }
        if dependencies.len() > 2048 {
            return Err(Error::Invalid("workflow dependency bound exceeded".into()));
        }
        sqlx::query("UPDATE model_results SET execution_telemetry=$7,dependencies=$8,mutation_operations=$10,proposal=CASE WHEN $6::jsonb IS NULL THEN COALESCE(proposal,$5) ELSE NULL END,outcome=COALESCE($6,outcome),context=CASE WHEN $6::jsonb IS NULL THEN CASE WHEN proposal IS NULL THEN COALESCE($9,context) ELSE context END ELSE NULL END,maintenance_need_id=COALESCE(maintenance_need_id,$11),maintenance_trigger_revision=COALESCE(maintenance_trigger_revision,$12),updated_at=now() WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 AND semantic_digest=$4 AND outcome IS NULL")
            .bind(subject.0).bind(owner).bind(key).bind(digest).bind(proposal.map(Json)).bind(outcome.map(Json))
            .bind(telemetry.as_ref().map(Json)).bind(Json(&dependencies)).bind(context.map(Json)).bind(operations).bind(maintenance.map(|claim| claim.need_id)).bind(revision)
            .execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }
}
