// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use nous_core::{CognitiveRef, Error, Result, SubjectId, reference_parts};
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
            return Err(Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::InvalidRequest,
                "invalid workflow owner identifier",
            ));
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

pub struct PendingWorkflow {
    pub snapshot: WorkflowSnapshot,
    pub proposal: Option<WorkflowPayload>,
    pub execution_telemetry: Option<ExecutionTelemetry>,
}

pub struct CompletedWorkflow {
    pub outcome: WorkflowPayload,
    pub execution_telemetry: Option<ExecutionTelemetry>,
}

pub enum WorkflowReservation {
    Missing,
    Acquired {
        pending: PendingWorkflow,
        lease: WorkflowLease,
    },
    Busy(PendingWorkflow),
    Completed(CompletedWorkflow),
}

fn completed_workflow(row: &sqlx::postgres::PgRow) -> Result<Option<CompletedWorkflow>> {
    let Some(outcome) = row
        .try_get::<Option<Json<WorkflowPayload>>, _>("outcome")
        .map_err(db)?
    else {
        return Ok(None);
    };
    Ok(Some(CompletedWorkflow {
        outcome: outcome.0,
        execution_telemetry: row
            .try_get::<Option<Json<ExecutionTelemetry>>, _>("execution_telemetry")
            .map_err(db)?
            .map(|value| value.0),
    }))
}

fn pending_workflow(row: &sqlx::postgres::PgRow) -> Result<PendingWorkflow> {
    Ok(PendingWorkflow {
        snapshot: row
            .try_get::<Json<WorkflowSnapshot>, _>("snapshot")
            .map_err(db)?
            .0,
        proposal: row
            .try_get::<Option<Json<WorkflowPayload>>, _>("proposal")
            .map_err(db)?
            .map(|value| value.0),
        execution_telemetry: row
            .try_get::<Option<Json<ExecutionTelemetry>>, _>("execution_telemetry")
            .map_err(db)?
            .map(|value| value.0),
    })
}

fn validate_open_request(
    key: &str,
    digest: &str,
    snapshot: Option<&WorkflowSnapshot>,
    lease_seconds: u64,
) -> Result<()> {
    if !(1..=ABSOLUTE_WORKFLOW_LEASE_SECONDS).contains(&lease_seconds)
        || key.is_empty()
        || key.len() > 256
        || digest.is_empty()
        || digest.len() > 128
    {
        return Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::InvalidRequest,
            "invalid model workflow reservation",
        ));
    }
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    if snapshot.content.dependencies.len() > 2048 {
        return Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::InvalidRequest,
            "invalid model workflow reservation",
        ));
    }
    let snapshot_size = serde_json::to_vec(snapshot)
        .map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::InvalidRequest,
                "Model workflow snapshot cannot be serialized",
            )
            .with_source(error)
        })?
        .len();
    if snapshot_size > 262144 {
        return Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::InvalidRequest,
            "invalid model workflow reservation",
        ));
    }
    Ok(())
}

async fn parent_maintenance_lease_until(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    claim: Option<&nous_core::MaintenanceClaim>,
    trigger_revision: Option<i64>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let Some(claim) = claim else {
        return Ok(None);
    };
    let until: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT lease_until FROM maintenance_needs WHERE subject_id=$1 AND need_id=$2 AND state='leased' AND lease_token=$3 AND lease_until>clock_timestamp() AND trigger_revision>=$4 FOR UPDATE")
        .bind(subject.0)
        .bind(claim.need_id)
        .bind(claim.lease_token)
        .bind(trigger_revision)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    Ok(Some(until.ok_or_else(|| {
        Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::LeaseLost,
            "Maintenance lease expired or changed",
        )
        .with_context("need_id", claim.need_id)
    })?))
}

async fn lock_workflow_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    owner: &str,
    key: &str,
    digest: &str,
    snapshot: Option<&WorkflowSnapshot>,
    maintenance_claim: Option<&nous_core::MaintenanceClaim>,
    maintenance_trigger: Option<i64>,
) -> Result<Option<sqlx::postgres::PgRow>> {
    if let Some(snapshot) = snapshot {
        sqlx::query("INSERT INTO model_workflow_operations(subject_id,owner,operation_key,semantic_digest,snapshot,maintenance_need_id,maintenance_trigger_revision,maintenance_lease_token,maintenance_trigger_authority_seq,dependencies) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT DO NOTHING")
            .bind(subject.0)
            .bind(owner)
            .bind(key)
            .bind(digest)
            .bind(Json(snapshot))
            .bind(maintenance_claim.map(|claim| claim.need_id))
            .bind(maintenance_trigger)
            .bind(maintenance_claim.map(|claim| claim.lease_token))
            .bind(maintenance_claim.map(|claim| claim.trigger_authority_seq))
            .bind(Json(&snapshot.content.dependencies))
            .execute(&mut **tx)
            .await
            .map_err(db)?;
    }
    let row = sqlx::query("SELECT snapshot,proposal,outcome,execution_telemetry,semantic_digest,maintenance_need_id,maintenance_trigger_revision,maintenance_lease_token,maintenance_trigger_authority_seq,lease_token,lease_token IS NOT NULL AND lease_until>clock_timestamp() AS live FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 FOR UPDATE")
        .bind(subject.0)
        .bind(owner)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>("semantic_digest").map_err(db)? != digest {
        return Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::OperationIdConflict,
            "Model operation identity has different semantic input",
        )
        .with_context("owner", owner)
        .with_context("operation_key", key));
    }
    Ok(Some(row))
}

fn peek_completed_workflow(
    row: &sqlx::postgres::PgRow,
    digest: &str,
    owner: &str,
    key: &str,
) -> Result<Option<CompletedWorkflow>> {
    if row.try_get::<String, _>("semantic_digest").map_err(db)? != digest {
        return Err(nous_core::Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::OperationIdConflict,
            "Model operation identity has different semantic input",
        )
        .with_context("owner", owner)
        .with_context("operation_key", key));
    }
    completed_workflow(row)
}

fn validate_checkpoint_values(
    proposal: Option<&WorkflowPayload>,
    outcome: Option<&WorkflowPayload>,
    execution_telemetry: Option<&ExecutionTelemetry>,
    mutation_operation_id: Option<nous_core::OperationId>,
) -> Result<()> {
    for value in [proposal, outcome].into_iter().flatten() {
        if serde_json::to_vec(value)
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::ModelWorkflow,
                    nous_core::ErrorCode::InvalidRequest,
                    "Model workflow payload cannot be serialized",
                )
                .with_source(error)
            })?
            .len()
            > WORKFLOW_VALUE_MAX_BYTES
        {
            return Err(Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::InvalidRequest,
                "model workflow value exceeds bound",
            ));
        }
    }
    if let Some(telemetry) = execution_telemetry {
        telemetry.validate()?;
    }
    if proposal.is_none()
        && outcome.is_none()
        && execution_telemetry.is_none()
        && mutation_operation_id.is_none()
    {
        return Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::InvalidRequest,
            "empty model workflow checkpoint",
        ));
    }
    Ok(())
}

async fn validated_maintenance_claim_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    owner: &str,
    key: &str,
) -> Result<Option<nous_core::MaintenanceClaim>> {
    let identity_row = sqlx::query("SELECT maintenance_need_id,maintenance_trigger_revision,maintenance_lease_token,maintenance_trigger_authority_seq FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
        .bind(subject.0)
        .bind(owner)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        .ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::LeaseLost,
                "Model workflow lease expired or changed",
            )
            .with_context("owner", owner)
            .with_context("operation_key", key)
        })?;
    let maintenance_need_id = identity_row
        .try_get::<Option<Uuid>, _>("maintenance_need_id")
        .map_err(db)?;
    let maintenance_trigger_revision = identity_row
        .try_get::<Option<i64>, _>("maintenance_trigger_revision")
        .map_err(db)?;
    let maintenance_lease_token = identity_row
        .try_get::<Option<Uuid>, _>("maintenance_lease_token")
        .map_err(db)?;
    let maintenance_trigger_authority_seq = identity_row
        .try_get::<Option<i64>, _>("maintenance_trigger_authority_seq")
        .map_err(db)?;
    match (
        maintenance_need_id,
        maintenance_trigger_revision,
        maintenance_lease_token,
        maintenance_trigger_authority_seq,
    ) {
        (Some(need_id), Some(trigger_revision), Some(lease_token), Some(trigger_authority_seq)) => {
            let trigger_revision_u64 = u64::try_from(trigger_revision).map_err(|_| {
                Error::new(
                    nous_core::ErrorOwner::ModelWorkflow,
                    nous_core::ErrorCode::InvalidRequest,
                    "maintenance revision exceeds storage range",
                )
            })?;
            let valid: Option<Uuid> = sqlx::query_scalar("SELECT need_id FROM maintenance_needs WHERE subject_id=$1 AND need_id=$2 AND state='leased' AND lease_token=$3 AND lease_until>clock_timestamp() AND trigger_revision>=$4 FOR UPDATE")
                .bind(subject.0)
                .bind(need_id)
                .bind(lease_token)
                .bind(trigger_revision)
                .fetch_optional(&mut **tx)
                .await
                .map_err(db)?;
            if valid.is_none() {
                return Err(Error::new(
                    nous_core::ErrorOwner::ModelWorkflow,
                    nous_core::ErrorCode::LeaseLost,
                    "Maintenance lease expired or changed",
                )
                .with_context("need_id", need_id));
            }
            Ok(Some(nous_core::MaintenanceClaim {
                need_id,
                lease_token,
                trigger_authority_seq,
                trigger_revision: trigger_revision_u64,
            }))
        }
        (None, None, None, None) => Ok(None),
        _ => Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::OperationIdConflict,
            "Model workflow maintenance identity changed",
        )
        .with_context("owner", owner)
        .with_context("operation_key", key)),
    }
}

impl AuthorityStore {
    pub async fn add_workflow_dependencies_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        owner: &str,
        operation: nous_core::OperationId,
        dependencies: &[nous_core::CognitiveRef],
    ) -> Result<()> {
        sqlx::query("UPDATE model_workflow_operations w SET dependencies=(SELECT COALESCE(jsonb_agg(DISTINCT ref),'[]') FROM jsonb_array_elements(w.dependencies||$4::jsonb) ref) WHERE w.subject_id=$1 AND w.owner=$2 AND (w.operation_key=$3::uuid::text OR w.mutation_operation_id=$3) AND w.outcome IS NULL")
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

    /// Clear context bodies that directly depended on purged owner refs while
    /// retaining an ended shell for existing longitudinal references.
    pub async fn purge_work_contexts_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<()> {
        if references.is_empty() {
            return Ok(());
        }
        let parts = references.iter().map(reference_parts).collect::<Vec<_>>();
        let (kinds, values): (Vec<_>, Vec<_>) = parts.into_iter().unzip();
        let affected: Vec<uuid::Uuid> = sqlx::query_scalar(
            r#"
WITH targets(ref_kind,ref_value) AS (
    SELECT * FROM unnest($2::text[],$3::text[])
)
SELECT DISTINCT wc.work_context_id
FROM work_contexts wc
JOIN work_context_refs wcr USING(work_context_id)
JOIN targets requested USING(ref_kind,ref_value)
WHERE wc.subject_id=$1
ORDER BY wc.work_context_id
"#,
        )
        .bind(subject.0)
        .bind(&kinds)
        .bind(&values)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
        for id in affected {
            let context = CognitiveRef::WorkContext(nous_core::WorkContextId(id));
            Self::tombstone_identity_refs_in(tx, subject, std::slice::from_ref(&context)).await?;
            Self::purge_owner_history_in(tx, subject, std::slice::from_ref(&context)).await?;
        }
        sqlx::query(
            r#"
WITH targets(ref_kind,ref_value) AS (
    SELECT * FROM unnest($2::text[],$3::text[])
)
UPDATE work_contexts wc SET
    state='ended',purpose='Purged context',context_text='',
    unresolved_questions='{}',constraints='{}',resume_conditions='{}',
    budget_summary='{}',revision=revision+1,
    ended_at=COALESCE(ended_at,clock_timestamp()),updated_at=clock_timestamp()
WHERE wc.subject_id=$1 AND EXISTS (
    SELECT 1 FROM work_context_refs wcr
    JOIN targets requested USING(ref_kind,ref_value)
    WHERE wcr.work_context_id=wc.work_context_id
)
"#,
        )
        .bind(subject.0)
        .bind(&kinds)
        .bind(&values)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        sqlx::query(
            r#"
WITH targets(ref_kind,ref_value) AS (
    SELECT * FROM unnest($2::text[],$3::text[])
), affected(work_context_id) AS (
    SELECT DISTINCT wc.work_context_id
    FROM work_contexts wc
    JOIN work_context_refs wcr USING(work_context_id)
    JOIN targets requested USING(ref_kind,ref_value)
    WHERE wc.subject_id=$1
)
DELETE FROM work_context_refs wcr USING affected
WHERE wcr.work_context_id=affected.work_context_id
"#,
        )
        .bind(subject.0)
        .bind(kinds)
        .bind(values)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn open_model_workflow(
        &self,
        subject: SubjectId,
        owner: &str,
        key: &str,
        digest: &str,
        snapshot: Option<&WorkflowSnapshot>,
        maintenance_claim: Option<&nous_core::MaintenanceClaim>,
        lease_seconds: u64,
    ) -> Result<WorkflowReservation> {
        let owner_id = WorkflowOwner::new(owner)?;
        let owner = owner_id.as_str();
        validate_open_request(key, digest, snapshot, lease_seconds)?;
        if snapshot.is_none() && maintenance_claim.is_some() {
            // Completed maintenance work is replayable even if the caller's
            // parent claim has expired. Check only its outcome before taking
            // the parent lease lock; pending work is re-read under both locks.
            let row = sqlx::query("SELECT semantic_digest,outcome,execution_telemetry FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
                .bind(subject.0)
                .bind(owner)
                .bind(key)
                .fetch_optional(self.pool())
                .await
                .map_err(db)?;
            let Some(row) = row else {
                return Ok(WorkflowReservation::Missing);
            };
            if let Some(completed) = peek_completed_workflow(&row, digest, owner, key)? {
                return Ok(WorkflowReservation::Completed(completed));
            }
        }
        let mut tx = self.begin().await?;
        let maintenance_trigger = maintenance_claim
            .map(|claim| i64::try_from(claim.trigger_revision))
            .transpose()
            .map_err(|_| {
                Error::new(
                    nous_core::ErrorOwner::ModelWorkflow,
                    nous_core::ErrorCode::InvalidRequest,
                    "maintenance revision exceeds storage range",
                )
            })?;
        let parent_lease_until = parent_maintenance_lease_until(
            &mut tx,
            subject,
            maintenance_claim,
            maintenance_trigger,
        )
        .await?;

        let row = lock_workflow_in(
            &mut tx,
            subject,
            owner,
            key,
            digest,
            snapshot,
            maintenance_claim,
            maintenance_trigger,
        )
        .await?;
        let Some(row) = row else {
            tx.commit().await.map_err(db)?;
            return Ok(WorkflowReservation::Missing);
        };
        if let Some(completed) = completed_workflow(&row)? {
            tx.commit().await.map_err(db)?;
            return Ok(WorkflowReservation::Completed(completed));
        }
        let pending = pending_workflow(&row)?;
        let stored_need = row
            .try_get::<Option<Uuid>, _>("maintenance_need_id")
            .map_err(db)?;
        let stored_trigger = row
            .try_get::<Option<i64>, _>("maintenance_trigger_revision")
            .map_err(db)?;
        let stored_authority_seq = row
            .try_get::<Option<i64>, _>("maintenance_trigger_authority_seq")
            .map_err(db)?;
        let stored_claim_token = row
            .try_get::<Option<Uuid>, _>("maintenance_lease_token")
            .map_err(db)?;
        match (
            maintenance_claim,
            stored_need,
            stored_trigger,
            stored_authority_seq,
            stored_claim_token,
        ) {
            (Some(claim), Some(need_id), Some(trigger), Some(authority_seq), Some(_))
                if claim.need_id == need_id
                    && i64::try_from(claim.trigger_revision).ok() == Some(trigger)
                    && claim.trigger_authority_seq == authority_seq => {}
            (None, None, None, None, None) => {}
            _ => {
                return Err(nous_core::Error::new(
                    nous_core::ErrorOwner::ModelWorkflow,
                    nous_core::ErrorCode::OperationIdConflict,
                    "Model workflow maintenance identity changed",
                )
                .with_context("owner", owner)
                .with_context("operation_key", key));
            }
        }
        let same_parent_claim = maintenance_claim
            .map(|claim| stored_claim_token == Some(claim.lease_token))
            .unwrap_or(true);
        if same_parent_claim && row.try_get::<bool, _>("live").map_err(db)? {
            tx.commit().await.map_err(db)?;
            return Ok(WorkflowReservation::Busy(pending));
        }
        let token = Uuid::now_v7();
        if let (Some(claim), Some(parent_until)) = (maintenance_claim, parent_lease_until) {
            sqlx::query("UPDATE model_workflow_operations SET maintenance_lease_token=$5,lease_token=$4,lease_until=LEAST($6::timestamptz,clock_timestamp()+($7::double precision * interval '1 second')),updated_at=clock_timestamp() WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
                    .bind(subject.0).bind(owner).bind(key).bind(token).bind(claim.lease_token).bind(parent_until).bind(lease_seconds as f64).execute(&mut *tx).await.map_err(db)?;
        } else {
            sqlx::query("UPDATE model_workflow_operations SET lease_token=$4,lease_until=clock_timestamp()+($5::double precision * interval '1 second'),updated_at=clock_timestamp() WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
                    .bind(subject.0).bind(owner).bind(key).bind(token).bind(lease_seconds as f64).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(WorkflowReservation::Acquired {
            pending,
            lease: WorkflowLease {
                subject,
                owner: owner_id,
                operation_key: key.into(),
                token,
            },
        })
    }

    pub async fn save_model_workflow(
        &self,
        lease: &WorkflowLease,
        proposal: Option<&WorkflowPayload>,
        outcome: Option<&WorkflowPayload>,
        execution_telemetry: Option<&ExecutionTelemetry>,
        mutation_operation_id: Option<nous_core::OperationId>,
    ) -> Result<()> {
        let WorkflowLease {
            subject,
            owner,
            operation_key: key,
            token,
        } = lease;
        let owner = owner.as_str();
        validate_checkpoint_values(
            proposal,
            outcome,
            execution_telemetry,
            mutation_operation_id,
        )?;
        let mut tx = self.begin().await?;
        let maintenance_claim =
            validated_maintenance_claim_in(&mut tx, *subject, owner, key).await?;
        let maintenance_claim = maintenance_claim.as_ref();
        let row = sqlx::query("SELECT dependencies,execution_telemetry,lease_token,maintenance_lease_token,lease_until>clock_timestamp() AS child_live,outcome IS NOT NULL AS terminal FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3 FOR UPDATE")
            .bind(subject.0).bind(owner).bind(key).fetch_optional(&mut *tx).await.map_err(db)?
            .ok_or_else(|| nous_core::Error::new(nous_core::ErrorOwner::ModelWorkflow, nous_core::ErrorCode::LeaseLost, "Model workflow lease expired or changed")
                .with_context("owner", owner).with_context("operation_key", key))?;
        if row.try_get::<Option<Uuid>, _>("lease_token").map_err(db)? != Some(*token)
            || !row.try_get::<bool, _>("child_live").map_err(db)?
            || row.try_get::<bool, _>("terminal").map_err(db)?
            || row
                .try_get::<Option<Uuid>, _>("maintenance_lease_token")
                .map_err(db)?
                != maintenance_claim.map(|claim| claim.lease_token)
        {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::LeaseLost,
                "Model workflow lease expired or changed",
            )
            .with_context("owner", owner)
            .with_context("operation_key", key));
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
            return Err(Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::InvalidRequest,
                "workflow dependency bound exceeded",
            ));
        }
        let updated = sqlx::query("UPDATE model_workflow_operations w SET execution_telemetry=$7,dependencies=$8,mutation_operation_id=$10,proposal=CASE WHEN $6::jsonb IS NULL THEN COALESCE($5,proposal) ELSE NULL END,outcome=COALESCE($6,outcome),snapshot=CASE WHEN $6::jsonb IS NULL THEN snapshot ELSE $9 END,lease_token=CASE WHEN $6::jsonb IS NULL THEN lease_token ELSE NULL END,lease_until=CASE WHEN $6::jsonb IS NULL THEN lease_until ELSE NULL END,updated_at=clock_timestamp() WHERE w.subject_id=$1 AND w.owner=$2 AND w.operation_key=$3 AND w.lease_token=$4 AND w.lease_until>clock_timestamp() AND w.outcome IS NULL AND (w.maintenance_need_id IS NULL OR EXISTS (SELECT 1 FROM maintenance_needs n WHERE n.subject_id=w.subject_id AND n.need_id=w.maintenance_need_id AND n.state='leased' AND n.lease_token=w.maintenance_lease_token AND n.lease_until>clock_timestamp() AND n.trigger_revision>=w.maintenance_trigger_revision))")
            .bind(subject.0)
            .bind(owner)
            .bind(key)
            .bind(*token)
            .bind(proposal.map(Json))
            .bind(outcome.map(Json))
            .bind(telemetry.as_ref().map(Json))
            .bind(Json(&dependencies))
            .bind(Json(WorkflowSnapshot::default()))
            .bind(mutation_operation_id.map(|operation| operation.0))
            .execute(&mut *tx).await.map_err(db)?;
        if updated.rows_affected() != 1 {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::LeaseLost,
                "Model workflow lease expired or changed",
            )
            .with_context("owner", owner)
            .with_context("operation_key", key));
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
