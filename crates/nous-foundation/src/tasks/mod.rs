//! Durable task authority, submission and cancellation.
mod cancellation;
mod config;
mod registry;
mod runtime;
mod wait;
use crate::clock::Clock;
pub use config::{TASK_RUNTIME, TaskRuntimeConfig, register_configuration};
use nous_domain::*;
use nous_storage::{BeginOperation, Database, WriteTransaction};
pub use registry::TaskRegistry;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Tasks {
    database: Database,
    clock: Clock,
    config: TaskRuntimeConfig,
    running: Arc<Mutex<HashMap<String, CancellationToken>>>,
    wake: Arc<tokio::sync::Notify>,
    changed: Arc<tokio::sync::Notify>,
}
impl Tasks {
    pub fn new(database: Database, clock: Clock, config: TaskRuntimeConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            database,
            clock,
            config,
            running: Arc::default(),
            wake: Arc::default(),
            changed: Arc::default(),
        })
    }
    pub fn new_job(
        &self,
        tx: &WriteTransaction,
        kind: &str,
        namespace: Option<NamespaceId>,
        subject: Option<SubjectId>,
    ) -> JobRecord {
        let now = tx.time().recorded();
        JobRecord {
            id: ulid::Ulid::generate().to_string(),
            trace_id: tx.context().trace_id.clone(),
            namespace_id: namespace,
            subject_id: subject,
            source_read: tx.context().source_read.clone(),
            kind: kind.into(),
            dependencies: Vec::new(),
            state: JobState::Queued,
            wait: None,
            checkpoint: None,
            result: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }
    pub fn notify_wake(&self) {
        self.wake.notify_one();
        self.changed.notify_waiters();
    }
    pub async fn submit_in(&self, tx: &mut WriteTransaction, job: &JobRecord) -> Result<()> {
        tx.insert_job_record(job, self.config.max_queue_attempts)
            .await?;
        self.observe_transaction(tx);
        Ok(())
    }
    pub async fn submit_after_in(
        &self,
        tx: &mut WriteTransaction,
        job: &mut JobRecord,
        dependencies: Vec<String>,
    ) -> Result<()> {
        job.dependencies = dependencies;
        if !job.dependencies.is_empty() {
            job.state = JobState::Waiting;
            job.wait = Some(WaitCondition {
                kind: "dependencies".into(),
                resume: ResumeCondition::Ready,
                details: json!({}),
            });
        }
        self.submit_in(tx, job).await
    }
    pub async fn dependencies(&self, job: &JobRecord) -> Result<Vec<JobRecord>> {
        let mut dependencies = Vec::with_capacity(job.dependencies.len());
        for id in &job.dependencies {
            dependencies.push(self.database.job_record(id).await?);
        }
        Ok(dependencies)
    }
    pub async fn read(&self, ctx: &CallContext, id: &str) -> Result<JobRecord> {
        let job = self.database.job_record(id).await?;
        authorize(ctx, &job)?;
        Ok(job)
    }
    pub async fn change_in(
        &self,
        tx: &mut WriteTransaction,
        job: &mut JobRecord,
        state: JobState,
        wait: Option<WaitCondition>,
        error: Option<Error>,
    ) -> Result<()> {
        job.state = state;
        job.wait = wait;
        job.error = error.map(|error| error.with_trace(job.trace_id.clone()));
        job.updated_at = tx.time().recorded();
        tx.update_job_record(job, self.config.max_queue_attempts)
            .await?;
        self.observe_transaction(tx);
        tracing::debug!(trace_id=%job.trace_id,job_id=%job.id,job_kind=%job.kind,state=?job.state,"task transition staged");
        Ok(())
    }
    pub async fn complete_in(
        &self,
        tx: &mut WriteTransaction,
        job: &mut JobRecord,
        result: Value,
    ) -> Result<()> {
        job.result = Some(result);
        self.change_in(tx, job, JobState::Succeeded, None, None)
            .await
    }
    pub async fn operation(&self, action: &str, id: &str) -> Result<WriteTransaction> {
        let job = self.database.job_record(id).await?;
        let ctx = job.call_context("nous-worker")?;
        let key = nous_storage::operation_key(
            &ctx,
            action,
            id,
            &ulid::Ulid::generate().to_string(),
            &json!({"job_id":id}),
        )?;
        match self.database.begin_operation(key).await? {
            BeginOperation::New(tx) => Ok(tx),
            BeginOperation::Replay(_) => unreachable!("fresh task operation"),
        }
    }
}

fn terminal(state: JobState) -> bool {
    matches!(
        state,
        JobState::Succeeded | JobState::Failed | JobState::Cancelled
    )
}
fn authorize(ctx: &CallContext, job: &JobRecord) -> Result<()> {
    let accepted = job.call_context("task-read")?.source_read_identity()?;
    if ctx
        .source_read_identity()?
        .iter()
        .any(|constraint| !accepted.contains(constraint))
    {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "task body belongs to a different accepted source-read scope",
        ));
    }
    match (&job.namespace_id, &job.subject_id) {
        (Some(ns), Some(subject)) => ctx.require_subject(ns, subject),
        (Some(ns), None) => ctx.require_namespace(ns),
        _ => ctx.require_admin(),
    }
}
