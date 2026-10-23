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
    pub async fn insert_job_record(&mut self, value: &JobRecord) -> Result<()> {
        value.validate()?;
        if value.state != JobState::Queued {
            return Err(Error::invalid("new task must be queued"));
        }
        if let Some(namespace) = &value.namespace_id {
            self.namespace(namespace).await?;
            if let Some(subject) = &value.subject_id {
                self.subject(namespace, subject).await?;
            }
        }
        sqlx::query("INSERT INTO task_records(job_id,namespace_id,subject_id,kind,state,wait,checkpoint,result,error,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
   .bind(&value.id).bind(value.namespace_id.as_ref().map(NamespaceId::as_str)).bind(value.subject_id.as_ref().map(SubjectId::as_str)).bind(&value.kind).bind(enum_name(&value.state)?).bind(json_option(&value.wait)?).bind(json_option(&value.checkpoint)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(&value.created_at).bind(&value.updated_at).execute(&mut *self.tx).await.map_err(write_error)?;
        self.enqueue_task(&value.id).await?;
        Ok(())
    }
    pub async fn update_job_record(&mut self, value: &JobRecord) -> Result<()> {
        value.validate()?;
        let old = self.job_record(&value.id).await?;
        if old.namespace_id != value.namespace_id
            || old.subject_id != value.subject_id
            || old.kind != value.kind
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
        if matches!(old.state, JobState::Waiting | JobState::Failed)
            && value.state == JobState::Queued
        {
            let changed=sqlx::query("UPDATE Jobs SET status='Pending',attempts=0,lock_by=NULL,lock_at=NULL,done_at=NULL,last_result=NULL,run_at=strftime('%s','now') WHERE id=? AND status IN('Done','Failed','Killed')").bind(&value.id).execute(&mut *self.tx).await.map_err(write_error)?.rows_affected();
            if changed == 0 {
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
        namespace_id: row
            .get::<Option<String>, _>("namespace_id")
            .map(NamespaceId),
        subject_id: row.get::<Option<String>, _>("subject_id").map(SubjectId),
        kind: row.get("kind"),
        state: parse_enum(row.get("state"))?,
        wait: parse_optional(row.get("wait"))?,
        checkpoint: parse_optional(row.get("checkpoint"))?,
        result: parse_optional(row.get("result"))?,
        error: parse_optional(row.get("error"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

impl crate::ReadView {
    pub async fn job_record(&mut self, id: &str) -> Result<JobRecord> {
        read(&mut self.tx, id).await
    }
}
