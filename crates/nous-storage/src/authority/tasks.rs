//! Generic accepted work; provider attempts are owned by ModelCalls.
use crate::{
    BeginOperation, Database, JOB_QUEUE, OperationKey, WriteTransaction, codec::*, not_found,
    storage_error, write_error,
};
use nous_domain::*;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
impl Database {
    pub async fn job_record(&self, id: &str) -> Result<JobRecord> {
        read(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }
    pub async fn waiting_jobs(&self) -> Result<Vec<JobRecord>> {
        sqlx::query("SELECT * FROM task_records WHERE state='waiting' ORDER BY job_id")
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(row)
            .collect()
    }
    /// The host owns the instance lock and has not started its worker.
    pub async fn recover_interrupted_jobs(&self) -> Result<u64> {
        let BeginOperation::New(mut tx) = self
            .begin_operation(OperationKey {
                caller_id: "nous.recovery".into(),
                trace_id: TraceId::new(),
                operation: "work.recover".into(),
                scope: "instance".into(),
                operation_id: ulid::Ulid::generate().to_string(),
                request_hash: "recover".into(),
            })
            .await?
        else {
            unreachable!("fresh recovery")
        };
        let workers: i64 = sqlx::query_scalar("SELECT count(*) FROM Workers WHERE worker_type=?")
            .bind(JOB_QUEUE)
            .fetch_one(&mut *tx.tx)
            .await
            .map_err(storage_error)?;
        let count=sqlx::query("UPDATE Jobs SET status='Pending',attempts=attempts+1,done_at=NULL,run_at=strftime('%s','now') WHERE job_type=? AND status IN('Running','Queued')").bind(JOB_QUEUE).execute(&mut *tx.tx).await.map_err(write_error)?.rows_affected();
        sqlx::query(
            "UPDATE Jobs SET lock_by=NULL,lock_at=NULL WHERE job_type=? AND lock_by IS NOT NULL",
        )
        .bind(JOB_QUEUE)
        .execute(&mut *tx.tx)
        .await
        .map_err(write_error)?;
        sqlx::query("DELETE FROM Workers WHERE worker_type=?")
            .bind(JOB_QUEUE)
            .execute(&mut *tx.tx)
            .await
            .map_err(write_error)?;
        if count == 0 && workers == 0 {
            tx.rollback().await?;
        } else {
            tx.commit(serde_json::json!({"recovered":count})).await?;
        }
        Ok(count)
    }
}
impl WriteTransaction {
    pub async fn job_record(&mut self, id: &str) -> Result<JobRecord> {
        read(&mut self.tx, id).await
    }
    pub async fn insert_job_record(
        &mut self,
        value: &JobRecord,
        max_queue_attempts: u32,
    ) -> Result<()> {
        value.validate()?;
        if value.trace_id != *self.trace_id() {
            return Err(
                Error::invalid("accepted task trace differs from its operation")
                    .with_trace(self.trace_id().clone()),
            );
        }
        if max_queue_attempts == 0 {
            return Err(Error::invalid("maximum queue attempts must be positive")
                .with_trace(self.trace_id().clone()));
        }
        validate_job_id(&value.id)?;
        if !matches!(value.state, JobState::Queued | JobState::Waiting) {
            return Err(Error::invalid("new task must be queued or waiting"));
        }
        if let Some(namespace) = &value.namespace_id {
            self.namespace(namespace).await?;
            if let Some(subject) = &value.subject_id {
                self.subject(namespace, subject).await?;
            }
        }
        for id in &value.dependencies {
            let dependency = self.job_record(id).await?;
            if dependency.namespace_id != value.namespace_id
                || dependency.subject_id != value.subject_id
            {
                return Err(Error::invalid("task dependency belongs to another scope"));
            }
        }
        sqlx::query("INSERT INTO task_records(job_id,namespace_id,subject_id,kind,dependencies,state,wait,checkpoint,result,error,created_at,updated_at,trace_id,accepted_commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
   .bind(&value.id).bind(value.namespace_id.as_ref().map(NamespaceId::as_str)).bind(value.subject_id.as_ref().map(SubjectId::as_str)).bind(&value.kind).bind(serde_json::to_string(&value.dependencies)?).bind(enum_name(&value.state)?).bind(json_option(&value.wait)?).bind(json_option(&value.checkpoint)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(&value.created_at).bind(&value.updated_at).bind(value.trace_id.as_str()).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
        if value.state == JobState::Queued {
            self.enqueue_task(&value.id, max_queue_attempts).await?;
        }
        self.record_task(&value.id);
        Ok(())
    }
    pub async fn update_job_record(
        &mut self,
        value: &JobRecord,
        max_queue_attempts: u32,
    ) -> Result<()> {
        value.validate()?;
        if max_queue_attempts == 0 {
            return Err(Error::invalid("maximum queue attempts must be positive")
                .with_trace(self.trace_id().clone()));
        }
        let old = self.job_record(&value.id).await?;
        if old.namespace_id != value.namespace_id
            || old.subject_id != value.subject_id
            || old.kind != value.kind
            || old.dependencies != value.dependencies
            || old.trace_id != value.trace_id
            || old.created_at != value.created_at
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "task identity is immutable",
            ));
        }
        if matches!(old.state, JobState::Succeeded | JobState::Cancelled)
            && old.state != value.state
        {
            return Err(Error::new(ErrorCode::Conflict, "task is terminal"));
        }
        sqlx::query("UPDATE Jobs SET max_attempts=? WHERE id=? AND job_type=?")
            .bind(i64::from(max_queue_attempts))
            .bind(&value.id)
            .bind(JOB_QUEUE)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        if matches!(old.state, JobState::Waiting | JobState::Failed)
            && value.state == JobState::Queued
        {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM Jobs WHERE id=? AND job_type=?")
                    .bind(&value.id)
                    .bind(JOB_QUEUE)
                    .fetch_optional(&mut *self.tx)
                    .await
                    .map_err(storage_error)?;
            if status.is_none() {
                self.enqueue_task(&value.id, max_queue_attempts).await?;
            } else if status
                .as_deref()
                .is_some_and(|state| matches!(state, "Done" | "Failed" | "Killed"))
            {
                sqlx::query("UPDATE Jobs SET status='Pending',attempts=0,lock_by=NULL,lock_at=NULL,done_at=NULL,last_result=NULL,run_at=strftime('%s','now') WHERE id=? AND job_type=?")
                    .bind(&value.id).bind(JOB_QUEUE).execute(&mut *self.tx).await.map_err(write_error)?;
            } else {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "waiting task has not been acknowledged by its active worker",
                ));
            }
        }
        sqlx::query("UPDATE task_records SET state=?,wait=?,checkpoint=?,result=?,error=?,updated_at=? WHERE job_id=?").bind(enum_name(&value.state)?).bind(json_option(&value.wait)?).bind(json_option(&value.checkpoint)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(&value.updated_at).bind(&value.id).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub(crate) async fn require_subject_job(&mut self, value: &JobRecord) -> Result<JobRecord> {
        let stored = self.job_record(&value.id).await?;
        if stored.namespace_id != value.namespace_id || stored.subject_id != value.subject_id {
            return Err(Error::invalid("role input belongs to another task scope"));
        }
        stored.subject_scope()?;
        if stored.state == JobState::Cancelled {
            return Err(Error::new(ErrorCode::Conflict, "task is cancelled"));
        }
        Ok(stored)
    }
}
pub(crate) async fn read(conn: &mut SqliteConnection, id: &str) -> Result<JobRecord> {
    row(sqlx::query("SELECT * FROM task_records WHERE job_id=?")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?)
}
fn row(row: SqliteRow) -> Result<JobRecord> {
    Ok(JobRecord {
        id: row.get("job_id"),
        trace_id: TraceId(row.get("trace_id")),
        namespace_id: row
            .get::<Option<String>, _>("namespace_id")
            .map(NamespaceId),
        subject_id: row.get::<Option<String>, _>("subject_id").map(SubjectId),
        kind: row.get("kind"),
        dependencies: serde_json::from_str(row.get("dependencies"))?,
        state: parse_enum(row.get("state"))?,
        wait: parse_optional(row.get("wait"))?,
        checkpoint: parse_optional(row.get("checkpoint"))?,
        result: parse_optional(row.get("result"))?,
        error: parse_optional(row.get("error"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub(crate) fn validate_job_id(id: &str) -> Result<()> {
    let parsed = id
        .parse::<ulid::Ulid>()
        .map_err(|_| Error::invalid("job ID must be a ULID"))?;
    if parsed.to_string() != id {
        return Err(Error::invalid("job ID must be canonical ULID"));
    }
    Ok(())
}

impl crate::ReadView {
    pub async fn job_record(&mut self, id: &str) -> Result<JobRecord> {
        read(&mut self.tx, id).await
    }
}
