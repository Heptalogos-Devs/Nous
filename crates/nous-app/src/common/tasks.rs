//! Durable task state and registered execution, independent of business handlers.
use super::clock::Clock;
use apalis::prelude::{BackendExt, Data, WorkerBuilder, WorkerBuilderExt};
use apalis_core::error::WorkerError;
use nous_domain::*;
use nous_storage::{BeginOperation, Database, OperationKey, WriteTransaction};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Tasks {
    database: Database,
    clock: Clock,
    running: Arc<Mutex<HashMap<String, CancellationToken>>>,
}
impl Tasks {
    pub fn new(database: Database, clock: Clock) -> Self {
        Self {
            database,
            clock,
            running: Arc::default(),
        }
    }
    pub fn new_job(
        &self,
        kind: &str,
        namespace: Option<NamespaceId>,
        subject: Option<SubjectId>,
    ) -> JobRecord {
        let now = self.clock.recorded();
        JobRecord {
            id: ulid::Ulid::generate().to_string(),
            namespace_id: namespace,
            subject_id: subject,
            kind: kind.into(),
            state: JobState::Queued,
            wait: None,
            checkpoint: None,
            result: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }
    pub async fn submit_in(&self, tx: &mut WriteTransaction, job: &JobRecord) -> Result<()> {
        tx.insert_job_record(job).await
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
        job.error = error;
        job.updated_at = self.clock.recorded();
        tx.update_job_record(job).await
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
    pub async fn cancel(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &str,
    ) -> Result<nous_storage::OperationReceipt> {
        let job = self.read(ctx, id).await?;
        let key =
            crate::operation_key(ctx, "task.cancel", id, operation_id, &json!({"job_id":id}))?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut current = tx.job_record(&job.id).await?;
        if !terminal(current.state) {
            self.change_in(&mut tx, &mut current, JobState::Cancelled, None, None)
                .await?;
        }
        let receipt = tx.commit(json!(current)).await?;
        if let Some(token) = self.running.lock().expect("task registry lock").get(id) {
            token.cancel();
        }
        Ok(receipt)
    }
    pub async fn operation(&self, action: &str, id: &str) -> Result<WriteTransaction> {
        let key = OperationKey {
            caller_id: "nous-worker".into(),
            operation: action.into(),
            scope: id.into(),
            operation_id: ulid::Ulid::generate().to_string(),
            request_hash: action.into(),
        };
        match self.database.begin_operation(key).await? {
            BeginOperation::New(tx) => Ok(tx),
            BeginOperation::Replay(_) => unreachable!("fresh task operation"),
        }
    }
    pub fn start(
        &self,
        registry: TaskRegistry,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let runtime = TaskRuntime {
            tasks: self.clone(),
            registry: Arc::new(registry),
            shutdown: shutdown.clone(),
        };
        tokio::spawn(async move {
            if let Err(error) = runtime.tasks.database.recover_interrupted_jobs().await {
                tracing::error!(%error,"cannot recover task queue");
                return;
            }
            if let Err(error) = runtime.wake_ready().await {
                tracing::warn!(%error,"waiting task recovery failed");
            }
            let maintenance = runtime.clone();
            let stop = shutdown.clone();
            let wake = async move {
                loop {
                    tokio::select! {_=stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(30))=>{
                        if let Err(error)=maintenance.wake_ready().await{tracing::warn!(%error,"waiting task wake failed");}
                    }}
                }
            };
            let backend = runtime
                .tasks
                .database
                .queue_backend()
                .poll_with_interval(Duration::from_millis(250));
            let worker = WorkerBuilder::new(format!("nous-{}", ulid::Ulid::generate()))
                .backend(backend)
                .data(runtime)
                .concurrency(1)
                .build(dispatch);
            let execute = async move {
                if let Err(error) = worker
                    .run_until(async move {
                        shutdown.cancelled().await;
                        Ok::<(), WorkerError>(())
                    })
                    .await
                {
                    tracing::error!(%error,"task runtime stopped");
                }
            };
            tokio::join!(execute, wake);
        })
    }
}

type TaskFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type Handler = Arc<dyn Fn(JobRecord, CancellationToken) -> TaskFuture + Send + Sync>;
type Readiness = Arc<dyn Fn(&WaitCondition) -> bool + Send + Sync>;
#[derive(Default)]
pub struct TaskRegistry {
    handlers: HashMap<String, (Handler, Readiness)>,
}
impl TaskRegistry {
    pub fn register<F, Fut, R>(&mut self, kind: &str, handler: F, ready: R) -> Result<()>
    where
        F: Fn(JobRecord, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
        R: Fn(&WaitCondition) -> bool + Send + Sync + 'static,
    {
        validate_name(kind, "task kind")?;
        if self.handlers.contains_key(kind) {
            return Err(Error::invalid("task handler is already registered"));
        }
        self.handlers.insert(
            kind.into(),
            (
                Arc::new(move |job, cancel| Box::pin(handler(job, cancel))),
                Arc::new(ready),
            ),
        );
        Ok(())
    }
}

#[derive(Clone)]
struct TaskRuntime {
    tasks: Tasks,
    registry: Arc<TaskRegistry>,
    shutdown: CancellationToken,
}
impl TaskRuntime {
    async fn wake_ready(&self) -> Result<()> {
        for job in self.tasks.database.waiting_jobs().await? {
            let Some(wait) = &job.wait else {
                continue;
            };
            let Some((_, ready)) = self.registry.handlers.get(&job.kind) else {
                continue;
            };
            let due = wait
                .details
                .get("not_before_ms")
                .and_then(Value::as_i64)
                .is_some_and(|at| self.tasks.clock.utc().timestamp_millis() >= at);
            if !due && wait.kind != "capability" && !ready(wait) {
                continue;
            }
            let mut tx = self.tasks.operation("task.wake", &job.id).await?;
            let mut current = tx.job_record(&job.id).await?;
            if current.state != JobState::Waiting {
                tx.rollback().await?;
                continue;
            }
            match self
                .tasks
                .change_in(&mut tx, &mut current, JobState::Queued, None, None)
                .await
            {
                Ok(()) => {
                    tx.commit(json!({"state":current.state})).await?;
                }
                Err(error) if error.code == ErrorCode::Conflict => {
                    tx.rollback().await?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    async fn execute(&self, id: &str) -> Result<()> {
        let mut tx = self.tasks.operation("task.start", id).await?;
        let mut job = tx.job_record(id).await?;
        if terminal(job.state) || job.state == JobState::Waiting {
            tx.rollback().await?;
            return Ok(());
        }
        let Some((handler, _)) = self.registry.handlers.get(&job.kind) else {
            let error = Error::new(
                ErrorCode::CapabilityUnavailable,
                "task handler is unavailable",
            );
            let wait = WaitCondition {
                kind: "capability".into(),
                details: json!({"task_kind":job.kind}),
            };
            self.tasks
                .change_in(
                    &mut tx,
                    &mut job,
                    JobState::Waiting,
                    Some(wait),
                    Some(error),
                )
                .await?;
            tx.commit(json!({"state":job.state})).await?;
            return Ok(());
        };
        self.tasks
            .change_in(&mut tx, &mut job, JobState::Running, None, None)
            .await?;
        tx.commit(json!({"state":job.state})).await?;
        let cancel = self.shutdown.child_token();
        self.tasks
            .running
            .lock()
            .expect("task registry lock")
            .insert(id.into(), cancel.clone());
        let result = handler(job, cancel).await;
        self.tasks
            .running
            .lock()
            .expect("task registry lock")
            .remove(id);
        if let Err(error) = result {
            let attempts = self.tasks.database.queue_record(id).await?.attempts;
            let mut tx = self.tasks.operation("task.failure", id).await?;
            let mut job = tx.job_record(id).await?;
            if terminal(job.state) || job.state == JobState::Waiting {
                tx.rollback().await?;
                return Ok(());
            }
            let retry = matches!(
                error.code,
                ErrorCode::Internal | ErrorCode::DependencyUnavailable
            ) && attempts < 5;
            self.tasks
                .change_in(
                    &mut tx,
                    &mut job,
                    if retry {
                        JobState::Queued
                    } else {
                        JobState::Failed
                    },
                    None,
                    Some(error.clone()),
                )
                .await?;
            tx.commit(json!({"state":job.state})).await?;
            if retry {
                return Err(error);
            }
        }
        Ok(())
    }
}
async fn dispatch(job: nous_storage::JobEnvelope, runtime: Data<TaskRuntime>) -> Result<()> {
    runtime.execute(&job.id).await
}
fn terminal(state: JobState) -> bool {
    matches!(
        state,
        JobState::Succeeded | JobState::Failed | JobState::Cancelled
    )
}
fn authorize(ctx: &CallContext, job: &JobRecord) -> Result<()> {
    match (&job.namespace_id, &job.subject_id) {
        (Some(ns), Some(subject)) => ctx.require_subject(ns, subject),
        (Some(ns), None) => ctx.require_namespace(ns),
        _ => ctx.require_admin(),
    }
}
