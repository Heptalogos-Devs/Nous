// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use nous_core::{DomainError, DomainErrorCode, Error, Result, SubjectId};
use nous_core::{ExecutionTelemetry, WorkflowPayload, WorkflowSnapshot};
use sqlx::Row;
use sqlx::{Postgres, Transaction, types::Json};
use uuid::Uuid;

/// Semantic owner identity; storage does not enumerate cognition domains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowOwner(String);
impl WorkflowOwner {
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
pub const ABSOLUTE_WORKFLOW_LEASE_SECONDS: u64 = 3600;
pub const WORKFLOW_VALUE_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WorkflowLease {
    pub subject: SubjectId,
    pub owner: WorkflowOwner,
    pub operation_key: String,
    pub token: Uuid,
}

pub struct WorkflowReservation {
    pub snapshot: WorkflowSnapshot,
    pub proposal: Option<WorkflowPayload>,
    pub outcome: Option<WorkflowPayload>,
    pub lease: Option<WorkflowLease>,
    pub busy: bool,
    pub execution_telemetry: Option<ExecutionTelemetry>,
}

impl AuthorityStore {
    pub async fn add_workflow_dependencies_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        owner: &str,
        operation: nous_core::OperationId,
        dependencies: &[nous_core::CognitiveRef],
    ) -> Result<()> {
        sqlx::query("UPDATE model_workflow_operations w SET dependencies=(SELECT COALESCE(jsonb_agg(DISTINCT ref),'[]') FROM jsonb_array_elements(w.dependencies||$4::jsonb) ref) WHERE w.subject_id=$1 AND w.owner=$2 AND (w.operation_key=$3::uuid::text OR $3=ANY(w.mutation_operations)) AND w.outcome IS NULL")
            .bind(subject.0).bind(owner).bind(operation.0).bind(Json(dependencies))
            .execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn workflow_cognitive_time_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        owner: &str,
        key: &str,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let value: Option<Json<WorkflowSnapshot>> = sqlx::query_scalar("SELECT snapshot FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
            .bind(subject.0).bind(owner).bind(key).fetch_optional(&mut **tx).await.map_err(db)?;
        Ok(value.and_then(|snapshot| snapshot.0.cognitive_formed_at))
    }

    pub async fn find_model_workflow(
        &self,
        subject: SubjectId,
        owner: &str,
        key: &str,
        digest: &str,
    ) -> Result<Option<WorkflowReservation>> {
        WorkflowOwner::new(owner)?;
        let row = sqlx::query("SELECT semantic_digest,snapshot,proposal,outcome,execution_telemetry FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
            .bind(subject.0).bind(owner).bind(key).fetch_optional(self.pool()).await.map_err(db)?;
        let Some(row) = row else { return Ok(None) };
        if row.try_get::<String, _>("semantic_digest").map_err(db)? != digest {
            return Err(DomainError::new(
                DomainErrorCode::OperationIdConflict,
                "Model operation identity has different semantic input",
            )
            .with_context("owner", owner)
            .with_context("operation_key", key)
            .into());
        }
        Ok(Some(WorkflowReservation {
            snapshot: row
                .try_get::<Json<WorkflowSnapshot>, _>("snapshot")
                .map_err(db)?
                .0,
            proposal: row
                .try_get::<Option<Json<WorkflowPayload>>, _>("proposal")
                .map_err(db)?
                .map(|v| v.0),
            outcome: row
                .try_get::<Option<Json<WorkflowPayload>>, _>("outcome")
                .map_err(db)?
                .map(|v| v.0),
            execution_telemetry: row
                .try_get::<Option<Json<ExecutionTelemetry>>, _>("execution_telemetry")
                .map_err(db)?
                .map(|v| v.0),
            lease: None,
            busy: false,
        }))
    }

    pub async fn purge_workflows_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        references: &[String],
    ) -> Result<()> {
        sqlx::query("UPDATE model_workflow_operations w SET snapshot=$3,proposal=NULL,outcome=$4,dependencies='[]',lease_token=NULL,lease_until=NULL,updated_at=clock_timestamp() WHERE subject_id=$1 AND EXISTS(SELECT 1 FROM jsonb_array_elements(w.dependencies) dependency WHERE dependency->>'id'=ANY($2::text[]))")
            .bind(subject.0).bind(references).bind(Json(WorkflowSnapshot::default()))
            .bind(Json(WorkflowPayload { purged: true, ..Default::default() }))
            .execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn reserve_model_workflow(
        &self,
        subject: SubjectId,
        owner: &str,
        key: &str,
        digest: &str,
        snapshot: &WorkflowSnapshot,
        lease_seconds: u64,
    ) -> Result<WorkflowReservation> {
        let owner_id = WorkflowOwner::new(owner)?;
        let owner = owner_id.as_str();
        if !(1..=ABSOLUTE_WORKFLOW_LEASE_SECONDS).contains(&lease_seconds)
            || snapshot.content.dependencies.len() > 2048
            || key.is_empty()
            || key.len() > 256
            || digest.is_empty()
            || digest.len() > 128
            || serde_json::to_vec(snapshot)
                .map_err(|error| Error::Invalid(error.to_string()))?
                .len()
                > 262144
        {
            return Err(Error::Invalid("invalid model workflow reservation".into()));
        }
        let mut tx = self.begin().await?;
        let maintenance = snapshot.maintenance_claim.as_ref();
        let maintenance_trigger = maintenance
            .map(|claim| i64::try_from(claim.trigger_revision))
            .transpose()
            .map_err(|_| Error::Invalid("maintenance revision exceeds storage range".into()))?;
        let (maintenance_need_id, maintenance_lease_until) = if let Some(claim) = maintenance {
            let until: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT lease_until FROM maintenance_needs WHERE subject_id=$1 AND need_id=$2 AND state='leased' AND lease_token=$3 AND lease_until>clock_timestamp() AND trigger_revision>=$4")
                .bind(subject.0).bind(claim.need_id).bind(claim.lease_token).bind(maintenance_trigger).fetch_optional(&mut *tx).await.map_err(db)?;
            let Some(until) = until else {
                return Err(DomainError::new(
                    DomainErrorCode::LeaseLost,
                    "Maintenance lease expired or changed",
                )
                .with_context("need_id", claim.need_id)
                .into());
            };
            (Some(claim.need_id), Some(until))
        } else {
            (None, None)
        };

        sqlx::query("INSERT INTO model_workflow_operations(subject_id,owner,operation_key,semantic_digest,snapshot,maintenance_need_id,maintenance_trigger_revision,dependencies) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING").bind(subject.0).bind(owner).bind(key).bind(digest).bind(Json(snapshot)).bind(maintenance_need_id).bind(maintenance_trigger).bind(Json(&snapshot.content.dependencies)).execute(&mut *tx).await.map_err(db)?;
        let row=sqlx::query("SELECT snapshot,proposal,outcome,execution_telemetry,semantic_digest,lease_until>now() AS live FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 FOR UPDATE").bind(subject.0).bind(owner).bind(key).fetch_one(&mut *tx).await.map_err(db)?;
        if row.try_get::<String, _>("semantic_digest").map_err(db)? != digest {
            return Err(DomainError::new(
                DomainErrorCode::OperationIdConflict,
                "Model operation identity has different semantic input",
            )
            .with_context("owner", owner)
            .with_context("operation_key", key)
            .into());
        }
        let snapshot = row
            .try_get::<Json<WorkflowSnapshot>, _>("snapshot")
            .map_err(db)?
            .0;
        let proposal = row
            .try_get::<Option<Json<WorkflowPayload>>, _>("proposal")
            .map_err(db)?
            .map(|v| v.0);
        let outcome = row
            .try_get::<Option<Json<WorkflowPayload>>, _>("outcome")
            .map_err(db)?
            .map(|v| v.0);
        let busy = outcome.is_none()
            && row
                .try_get::<Option<bool>, _>("live")
                .map_err(db)?
                .unwrap_or(false);
        let token = if outcome.is_none() && !busy {
            Some(Uuid::now_v7())
        } else {
            None
        };
        if let Some(token) = token {
            // The enclosing, validated maintenance claim already authorizes
            // this opportunity. A shorter general workflow default must not
            // expire its model proposal before the parent can acknowledge it.
            sqlx::query("UPDATE model_workflow_operations SET lease_token=$4,lease_until=GREATEST(clock_timestamp()+($5::double precision * interval '1 second'),$6::timestamptz),updated_at=now() WHERE subject_id=$1 AND owner=$2 AND operation_key=$3").bind(subject.0).bind(owner).bind(key).bind(token).bind(lease_seconds as f64).bind(maintenance_lease_until).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(WorkflowReservation {
            snapshot,
            proposal,
            outcome,
            lease: token.map(|token| WorkflowLease {
                subject,
                owner: owner_id,
                operation_key: key.into(),
                token,
            }),
            busy,
            execution_telemetry: row
                .try_get::<Option<Json<ExecutionTelemetry>>, _>("execution_telemetry")
                .map_err(db)?
                .map(|v| v.0),
        })
    }

    pub async fn save_model_workflow(
        &self,
        lease: &WorkflowLease,
        proposal: Option<&WorkflowPayload>,
        outcome: Option<&WorkflowPayload>,
        execution_telemetry: Option<&ExecutionTelemetry>,
        mutation_operations: &[nous_core::OperationId],
    ) -> Result<()> {
        let WorkflowLease {
            subject,
            owner,
            operation_key: key,
            token,
        } = lease;
        let owner = owner.as_str();
        for value in [proposal, outcome].into_iter().flatten() {
            if serde_json::to_vec(value)
                .map_err(|error| Error::Invalid(error.to_string()))?
                .len()
                > WORKFLOW_VALUE_MAX_BYTES
            {
                return Err(Error::Invalid("model workflow value exceeds bound".into()));
            }
        }
        if let Some(telemetry) = execution_telemetry {
            telemetry.validate()?;
        }
        let mut tx = self.begin().await?;
        let row = sqlx::query("SELECT dependencies,mutation_operations,execution_telemetry FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 AND lease_token=$4 AND lease_until>clock_timestamp() AND outcome IS NULL FOR UPDATE")
            .bind(subject.0).bind(owner).bind(key).bind(*token).fetch_optional(&mut *tx).await.map_err(db)?
            .ok_or_else(|| DomainError::new(DomainErrorCode::LeaseLost, "Model workflow lease expired or changed")
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
        for reference in [proposal, outcome]
            .into_iter()
            .flatten()
            .flat_map(|value| &value.dependencies)
        {
            if !dependencies.contains(reference) {
                dependencies.push(reference.clone());
            }
        }
        if dependencies.len() > 2048 {
            return Err(Error::Invalid("workflow dependency bound exceeded".into()));
        }
        let updated = sqlx::query("UPDATE model_workflow_operations SET execution_telemetry=$7,dependencies=$8,mutation_operations=$10,proposal=CASE WHEN $6::jsonb IS NULL THEN COALESCE($5,proposal) ELSE NULL END,outcome=COALESCE($6,outcome),snapshot=CASE WHEN $6::jsonb IS NULL THEN snapshot ELSE $9 END,lease_token=CASE WHEN $6::jsonb IS NULL THEN lease_token ELSE NULL END,lease_until=CASE WHEN $6::jsonb IS NULL THEN lease_until ELSE NULL END,updated_at=now() WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 AND lease_token=$4 AND lease_until>clock_timestamp() AND outcome IS NULL")
            .bind(subject.0).bind(owner).bind(key).bind(*token).bind(proposal.map(Json)).bind(outcome.map(Json))
            .bind(telemetry.as_ref().map(Json)).bind(Json(&dependencies)).bind(Json(WorkflowSnapshot::default())).bind(operations)
            .execute(&mut *tx).await.map_err(db)?;
        if updated.rows_affected() != 1 {
            return Err(DomainError::new(
                DomainErrorCode::LeaseLost,
                "Model workflow lease expired or changed",
            )
            .with_context("owner", owner)
            .with_context("operation_key", key)
            .into());
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    pub async fn release_model_workflow(&self, lease: &WorkflowLease) -> Result<()> {
        sqlx::query("UPDATE model_workflow_operations SET lease_token=NULL,lease_until=NULL,updated_at=now() WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 AND lease_token=$4 AND outcome IS NULL")
            .bind(lease.subject.0).bind(lease.owner.as_str()).bind(&lease.operation_key).bind(lease.token)
            .execute(self.pool()).await.map_err(db)?;
        Ok(())
    }
}
