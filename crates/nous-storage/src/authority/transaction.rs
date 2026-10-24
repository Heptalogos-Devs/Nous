use apalis::prelude::{TaskBuilder, TaskId};
use apalis_sqlite::queries::push_tasks;
use nous_domain::{
    Error, ErrorCode, Material, MaterialId, Namespace, NamespaceId, Observation, ObservationId,
    ObservationKind, Result, Subject, SubjectId, TraceId,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};
use std::sync::Arc;

use crate::{
    Database, JOB_QUEUE, JobEnvelope, OperationKey, OperationReceipt, not_found, storage_error,
    write_error,
};

#[derive(Debug)]
pub enum BeginOperation {
    Replay(OperationReceipt),
    New(WriteTransaction),
}

/// Owns the sole commit path for one application operation. Dropping it rolls
/// back authority changes, its commit sequence allocation, receipt, and jobs.
#[derive(Debug)]
pub struct WriteTransaction {
    pub(crate) tx: Transaction<'static, Sqlite>,
    key: Arc<OperationKey>,
    commit_seq: i64,
    job_ids: Vec<String>,
    lifecycle: OperationLifecycle,
    retained_guards: Vec<Box<dyn std::fmt::Debug + Send + Sync>>,
}

impl Database {
    pub async fn begin_operation(&self, key: OperationKey) -> Result<BeginOperation> {
        let key = Arc::new(key);
        key.trace_id.validate()?;
        // Acquire the writer before reading the receipt so concurrent retries
        // serialize and see the committed receipt rather than upgrade a snapshot.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|error| operation_error(storage_error(error), &key, None))?;
        let stored = sqlx::query("SELECT request_hash, receipt FROM operation_receipts WHERE caller_id=? AND operation=? AND scope=? AND operation_id=?")
            .bind(&key.caller_id).bind(&key.operation).bind(&key.scope).bind(&key.operation_id)
            .fetch_optional(&mut *tx).await.map_err(|error| operation_error(storage_error(error), &key, None))?;
        if let Some(row) = stored {
            if row.get::<String, _>("request_hash") != key.request_hash {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "operation ID was already used with a different request",
                )
                .with_trace(key.trace_id.clone())
                .with_context("operation_id", key.operation_id.clone()));
            }
            let receipt: OperationReceipt = serde_json::from_str(row.get("receipt"))
                .map_err(|error| operation_error(storage_error(error), &key, None))?;
            tx.rollback()
                .await
                .map_err(|error| operation_error(storage_error(error), &key, None))?;
            operation_event("replayed", &key, receipt.commit_seq);
            tracing::info!(event="operation.replay_origin", trace_id=%key.trace_id, original_trace_id=%receipt.trace_id, operation_id=%key.operation_id, commit_seq=receipt.commit_seq);
            return Ok(BeginOperation::Replay(receipt));
        }
        let commit_seq = sqlx::query_scalar("UPDATE authority_state SET commit_seq=commit_seq+1 WHERE singleton=1 RETURNING commit_seq")
            .fetch_one(&mut *tx).await.map_err(|error| operation_error(storage_error(error), &key, None))?;
        operation_event("started", &key, commit_seq);
        Ok(BeginOperation::New(WriteTransaction {
            tx,
            lifecycle: OperationLifecycle {
                key: key.clone(),
                commit_seq,
                finished: false,
            },
            key,
            commit_seq,
            job_ids: vec![],
            retained_guards: vec![],
        }))
    }
}

impl WriteTransaction {
    pub fn retain_until_end(&mut self, guard: impl std::fmt::Debug + Send + Sync + 'static) {
        self.retained_guards.push(Box::new(guard));
    }

    pub fn trace_id(&self) -> &TraceId {
        &self.key.trace_id
    }

    pub fn commit_seq(&self) -> i64 {
        self.commit_seq
    }

    pub async fn namespace(&mut self, id: &NamespaceId) -> Result<Namespace> {
        crate::authority::records::namespace(&mut self.tx, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn subject(&mut self, namespace: &NamespaceId, id: &SubjectId) -> Result<Subject> {
        crate::authority::records::subject(&mut self.tx, namespace, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn material(&mut self, namespace: &NamespaceId, id: &MaterialId) -> Result<Material> {
        crate::authority::records::material(&mut self.tx, namespace, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn observation(
        &mut self,
        namespace: &NamespaceId,
        id: &ObservationId,
    ) -> Result<Observation> {
        crate::authority::records::observation(&mut self.tx, namespace, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn insert_namespace(&mut self, value: &Namespace) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        sqlx::query("INSERT INTO namespaces(id,name,revision,commit_seq) VALUES (?,?,?,?)")
            .bind(value.id.as_str())
            .bind(&value.name)
            .bind(value.revision)
            .bind(self.commit_seq)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }

    pub async fn insert_subject(&mut self, value: &Subject) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        sqlx::query("INSERT INTO subjects(id,namespace_id,name,capabilities,revision,commit_seq) VALUES (?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(&value.name)
            .bind(serde_json::to_string(&value.capabilities).map_err(storage_error)?).bind(value.revision).bind(self.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn insert_material(&mut self, value: &Material) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        sqlx::query("INSERT INTO materials(id,namespace_id,content_hash,media_type,byte_len,source,event_time_domain,event_time_coordinate,recorded_at,commit_seq) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(&value.content_hash).bind(&value.media_type).bind(value.byte_len)
            .bind(&value.source).bind(value.event_time.as_ref().map(|t| t.domain.as_str())).bind(value.event_time.as_ref().map(|t| t.coordinate))
            .bind(&value.recorded_at).bind(self.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn insert_observation(&mut self, value: &Observation) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        let kind = match &value.kind {
            ObservationKind::Contact => "contact",
            ObservationKind::Import => "import",
        };
        sqlx::query("INSERT INTO observations(id,namespace_id,subject_id,material_id,observed_at_domain,observed_at_coordinate,recorded_at,commit_seq,kind) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.material_id.as_str())
            .bind(value.observed_at.as_ref().map(|t| t.domain.as_str())).bind(value.observed_at.as_ref().map(|t| t.coordinate))
            .bind(&value.recorded_at).bind(self.commit_seq).bind(kind).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub(crate) async fn enqueue_task(
        &mut self,
        id: &str,
        max_queue_attempts: u32,
    ) -> Result<String> {
        super::tasks::validate_job_id(id)?;
        if max_queue_attempts == 0 {
            return Err(Error::invalid("maximum queue attempts must be positive")
                .with_trace(self.trace_id().clone()));
        }
        let task = TaskBuilder::new(serde_json::to_vec(&JobEnvelope { id: id.into() })?)
            .task_id(TaskId::from(id.to_owned()))
            .max_attempts(max_queue_attempts as usize)
            .build();
        push_tasks(&mut *self.tx, JOB_QUEUE, &[task])
            .await
            .map_err(|error| match error {
                apalis_sqlite::Error::Database(error) => write_error(error),
                error => storage_error(error),
            })?;
        Ok(id.into())
    }

    pub(crate) fn record_task(&mut self, id: &str) {
        self.job_ids.push(id.into());
    }

    pub async fn commit(mut self, result: Value) -> Result<OperationReceipt> {
        let receipt = OperationReceipt {
            operation_id: self.key.operation_id.clone(),
            trace_id: self.key.trace_id.clone(),
            commit_seq: self.commit_seq,
            result,
            job_ids: self.job_ids,
        };
        sqlx::query("INSERT INTO operation_receipts(caller_id,operation,scope,operation_id,request_hash,commit_seq,receipt,trace_id) VALUES (?,?,?,?,?,?,?,?)")
            .bind(&self.key.caller_id).bind(&self.key.operation).bind(&self.key.scope).bind(&self.key.operation_id)
            .bind(&self.key.request_hash).bind(self.commit_seq).bind(serde_json::to_string(&receipt).map_err(|error| operation_error(storage_error(error), &self.key, Some(self.commit_seq)))?)
            .bind(self.key.trace_id.as_str())
            .execute(&mut *self.tx).await.map_err(|error| operation_error(write_error(error), &self.key, Some(self.commit_seq)))?;
        self.tx.commit().await.map_err(|error| {
            operation_error(storage_error(error), &self.key, Some(self.commit_seq))
        })?;
        operation_event("committed", &self.key, self.commit_seq);
        self.lifecycle.finished = true;
        Ok(receipt)
    }

    pub async fn rollback(mut self) -> Result<()> {
        self.tx.rollback().await.map_err(|error| {
            operation_error(storage_error(error), &self.key, Some(self.commit_seq))
        })?;
        operation_event("rolled_back", &self.key, self.commit_seq);
        self.lifecycle.finished = true;
        Ok(())
    }

    pub(crate) fn check_sequence(&self, sequence: i64) -> Result<()> {
        if sequence != self.commit_seq {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                "object commit sequence must match its transaction",
            ));
        }
        Ok(())
    }
}

pub fn operation_key(
    ctx: &nous_domain::CallContext,
    operation: &str,
    scope: &str,
    operation_id: &str,
    payload: &impl serde::Serialize,
) -> nous_domain::Result<OperationKey> {
    nous_domain::validate_name(operation_id, "operation_id")?;
    nous_domain::validate_name(&ctx.caller_id, "caller_id")?;
    let encoded =
        serde_json::to_vec(&json!({"operation":operation,"scope":scope,"payload":payload}))?;
    Ok(OperationKey {
        caller_id: ctx.caller_id.clone(),
        trace_id: ctx.trace_id.clone(),
        operation: operation.into(),
        scope: scope.into(),
        operation_id: operation_id.into(),
        request_hash: format!("{:x}", Sha256::digest(encoded)),
    })
}

fn operation_error(error: Error, key: &OperationKey, commit_seq: Option<i64>) -> Error {
    let mut error = error
        .with_trace(key.trace_id.clone())
        .with_context("operation", key.operation.clone())
        .with_context("operation_id", key.operation_id.clone());
    if let Some(sequence) = commit_seq {
        error = error.with_context("commit_seq", sequence);
    }
    error
}

fn operation_event(state: &'static str, key: &OperationKey, commit_seq: i64) {
    let event = match state {
        "started" => "operation.started",
        "replayed" => "operation.replayed",
        "committed" => "operation.committed",
        "rolled_back" => "operation.rolled_back",
        "abandoned" => "operation.abandoned",
        _ => unreachable!("known operation event"),
    };
    tracing::info!(event, trace_id=%key.trace_id, operation=%key.operation,
        operation_id=%key.operation_id, scope=%key.scope, commit_seq);
    metrics::counter!("nous_operations_total", "operation" => key.operation.clone(), "state" => state).increment(1);
}

#[derive(Debug)]
struct OperationLifecycle {
    key: Arc<OperationKey>,
    commit_seq: i64,
    finished: bool,
}
impl Drop for OperationLifecycle {
    fn drop(&mut self) {
        if !self.finished {
            operation_event("abandoned", &self.key, self.commit_seq);
        }
    }
}
