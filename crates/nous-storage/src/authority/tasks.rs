//! Generic accepted work; provider attempts are owned by ModelCalls.
use crate::{
    BeginOperation, Database, JOB_QUEUE, OperationKey, WriteTransaction, codec::*, not_found,
    storage_error, write_error,
};
use nous_domain::*;
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, sqlite::SqliteRow};
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
                context: CallContext::new("nous.recovery", AccessScope::Admin),
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
        let count=sqlx::query("UPDATE Jobs SET status='Pending',attempts=attempts+1,done_at=NULL,run_at=? WHERE job_type=? AND status IN('Running','Queued')").bind(tx.time().utc().timestamp()).bind(JOB_QUEUE).execute(&mut *tx.tx).await.map_err(write_error)?.rows_affected();
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
        if value.trace_id != self.context().trace_id {
            return Err(
                Error::invalid("accepted task trace differs from its operation")
                    .with_trace(self.context().trace_id.clone()),
            );
        }
        if max_queue_attempts == 0 {
            return Err(Error::invalid("maximum queue attempts must be positive")
                .with_trace(self.context().trace_id.clone()));
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
        sqlx::query("INSERT INTO task_records(job_id,namespace_id,subject_id,source_read,kind,dependencies,dependency_policy,state,wait,checkpoint,result,error,created_at,updated_at,trace_id,accepted_commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
   .bind(&value.id).bind(value.namespace_id.as_ref().map(NamespaceId::as_str)).bind(value.subject_id.as_ref().map(SubjectId::as_str)).bind(serde_json::to_string(&value.source_read)?).bind(&value.kind).bind(serde_json::to_string(&value.dependencies)?).bind(enum_name(&value.dependency_policy)?).bind(enum_name(&value.state)?).bind(json_option(&value.wait)?).bind(json_option(&value.checkpoint)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(&value.created_at).bind(&value.updated_at).bind(value.trace_id.as_str()).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
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
                .with_trace(self.context().trace_id.clone()));
        }
        let old = self.job_record(&value.id).await?;
        if old.namespace_id != value.namespace_id
            || old.subject_id != value.subject_id
            || old.kind != value.kind
            || old.dependencies != value.dependencies
            || old.dependency_policy != value.dependency_policy
            || old.trace_id != value.trace_id
            || old.source_read != value.source_read
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
                sqlx::query("UPDATE Jobs SET status='Pending',attempts=0,lock_by=NULL,lock_at=NULL,done_at=NULL,last_result=NULL,run_at=? WHERE id=? AND job_type=?")
                    .bind(self.time().utc().timestamp()).bind(&value.id).bind(JOB_QUEUE).execute(&mut *self.tx).await.map_err(write_error)?;
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
        source_read: serde_json::from_str(row.get("source_read"))?,
        kind: row.get("kind"),
        dependencies: serde_json::from_str(row.get("dependencies"))?,
        dependency_policy: parse_enum(row.get("dependency_policy"))?,
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

impl crate::ReadView {
    pub async fn readable_job(&mut self, ctx: &CallContext, id: &str) -> Result<JobRecord> {
        validate_job_id(id)?;
        let job = self.job_record(id).await?;
        job.require_read(ctx)?;
        Ok(job)
    }
    pub async fn list_jobs(
        &mut self,
        ctx: &CallContext,
        request: &JobListRequest,
    ) -> Result<JobPage> {
        let filter = self.normalize_job_filter(ctx, request).await?;
        let filter_hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&serde_json::json!({
                "namespace_id":filter.namespace_id,"subject_id":filter.subject_id,"kind":filter.kind,
                "state":filter.state,"scope":ctx.scope,"source_read":ctx.source_read_identity()?,
            }))?)
        );
        let snapshot_seq = if let Some(cursor) = &request.cursor {
            validate_job_id(&cursor.before_job_id)?;
            if cursor.filter_hash != filter_hash
                || cursor.snapshot_seq < 0
                || cursor.snapshot_seq > self.watermark
                || cursor.before_commit_seq <= 0
                || cursor.before_commit_seq > cursor.snapshot_seq
            {
                return Err(Error::invalid(
                    "task cursor does not match its filters, authorization, or creation cutoff",
                ));
            }
            cursor.snapshot_seq
        } else {
            self.watermark
        };
        let mut before = request
            .cursor
            .as_ref()
            .map(|c| (c.before_commit_seq, c.before_job_id.clone()));
        let batch = (filter.limit + 1).max(64);
        let mut items = Vec::with_capacity(filter.limit + 1);
        loop {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT job_id,namespace_id,subject_id,source_read,trace_id,kind,state,accepted_commit_seq,dependency_policy,json_extract(wait,'$.kind') AS waiting_for,error,created_at,updated_at FROM task_records WHERE accepted_commit_seq<=",
            );
            query.push_bind(snapshot_seq);
            if let Some(namespace) = &filter.namespace_id {
                query
                    .push(" AND namespace_id=")
                    .push_bind(namespace.as_str());
            }
            if let Some(subject) = &filter.subject_id {
                query.push(" AND subject_id=").push_bind(subject.as_str());
            }
            if let Some(kind) = &filter.kind {
                query.push(" AND kind=").push_bind(kind);
            }
            if let Some(state) = filter.state {
                query.push(" AND state=").push_bind(enum_name(&state)?);
            }
            if let Some((seq, id)) = &before {
                query
                    .push(" AND (accepted_commit_seq,job_id)<(")
                    .push_bind(*seq)
                    .push(",")
                    .push_bind(id.clone())
                    .push(")");
            }
            query
                .push(" ORDER BY accepted_commit_seq DESC,job_id DESC LIMIT ")
                .push_bind(batch as i64);
            let rows = query
                .build()
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
            let count = rows.len();
            for row in rows {
                let id: String = row.get("job_id");
                let sequence: i64 = row.get("accepted_commit_seq");
                before = Some((sequence, id.clone()));
                let scope = JobReadScope {
                    trace_id: TraceId(row.get("trace_id")),
                    namespace_id: row
                        .get::<Option<String>, _>("namespace_id")
                        .map(NamespaceId),
                    subject_id: row.get::<Option<String>, _>("subject_id").map(SubjectId),
                    source_read: serde_json::from_str(row.get("source_read"))?,
                };
                match scope.require_read(ctx) {
                    Ok(()) => {}
                    Err(error) if error.code == ErrorCode::Forbidden => continue,
                    Err(error) => return Err(error),
                }
                items.push(JobSummary {
                    id,
                    trace_id: scope.trace_id,
                    namespace_id: scope.namespace_id,
                    subject_id: scope.subject_id,
                    kind: row.get("kind"),
                    state: parse_enum(row.get("state"))?,
                    accepted_commit_seq: sequence,
                    dependency_policy: parse_enum(row.get("dependency_policy"))?,
                    waiting_for: row.get("waiting_for"),
                    error: parse_optional(row.get("error"))?,
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                });
                if items.len() > filter.limit {
                    break;
                }
            }
            if items.len() > filter.limit || count < batch {
                break;
            }
        }
        let has_more = items.len() > filter.limit;
        if has_more {
            items.pop();
        }
        let next_cursor = if has_more {
            let last = items.last().expect("positive task page limit");
            Some(JobCursor {
                snapshot_seq,
                before_commit_seq: last.accepted_commit_seq,
                before_job_id: last.id.clone(),
                filter_hash,
            })
        } else {
            None
        };
        Ok(JobPage {
            snapshot_seq,
            read_at_commit_seq: self.watermark,
            items,
            next_cursor,
        })
    }
    async fn normalize_job_filter(
        &mut self,
        ctx: &CallContext,
        request: &JobListRequest,
    ) -> Result<JobListRequest> {
        if !(1..=200).contains(&request.limit) {
            return Err(Error::invalid("task list limit must be 1..200"));
        }
        if let Some(namespace) = &request.namespace_id {
            namespace.validate()?;
        }
        if let Some(subject) = &request.subject_id {
            subject.validate()?;
        }
        if let Some(kind) = &request.kind {
            validate_name(kind, "task kind")?;
        }
        let mut filter = request.clone();
        match &ctx.scope {
            AccessScope::Admin => {}
            AccessScope::Namespace { namespace_id } => {
                if let Some(requested) = &filter.namespace_id {
                    ctx.require_namespace(requested)?;
                }
                filter.namespace_id = Some(namespace_id.clone());
            }
            AccessScope::Subject {
                namespace_id,
                subject_id,
            } => {
                ctx.require_subject(
                    filter.namespace_id.as_ref().unwrap_or(namespace_id),
                    filter.subject_id.as_ref().unwrap_or(subject_id),
                )?;
                filter.namespace_id = Some(namespace_id.clone());
                filter.subject_id = Some(subject_id.clone());
            }
        }
        if let Some(subject) = &filter.subject_id {
            let namespace = self.subject_namespace(subject).await?;
            ctx.require_subject(&namespace, subject)?;
            if filter
                .namespace_id
                .as_ref()
                .is_some_and(|requested| requested != &namespace)
            {
                return Err(Error::invalid(
                    "task subject does not belong to the requested namespace",
                ));
            }
            filter.namespace_id = Some(namespace);
        } else if let Some(namespace) = &filter.namespace_id {
            self.namespace(namespace).await?;
        }
        Ok(filter)
    }
}
