//! Queue execution and recovery use the instance's frozen task policy.
use super::{
    TaskRegistry, Tasks,
    dependencies::{DependencyStatus, dependency_wait},
    terminal,
};
use apalis::prelude::{BackendExt, Data, WorkerBuilder, WorkerBuilderExt};
use apalis_core::error::WorkerError;
use nous_domain::{Error, ErrorCode, JobState, Result, ResumeCondition, WaitCondition};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

impl Tasks {
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
                    tokio::select! {
                        _=stop.cancelled()=>break,
                        _=tokio::time::sleep(Duration::from_millis(maintenance.tasks.config.wake_interval_ms))=>{},
                        _=maintenance.tasks.wake.notified()=>{},
                    }
                    if let Err(error) = maintenance.wake_ready().await {
                        tracing::warn!(%error,"waiting task wake failed");
                    }
                }
            };
            let backend = runtime
                .tasks
                .database
                .queue_backend(runtime.tasks.wake.clone())
                .poll_with_interval(Duration::from_millis(runtime.tasks.config.poll_interval_ms));
            let concurrency = runtime.tasks.config.concurrency;
            let worker = WorkerBuilder::new(format!("nous-{}", ulid::Ulid::generate()))
                .backend(backend)
                .data(runtime)
                .concurrency(concurrency)
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

#[derive(Clone)]
struct TaskRuntime {
    tasks: Tasks,
    registry: Arc<TaskRegistry>,
    shutdown: CancellationToken,
}
impl TaskRuntime {
    async fn wake_ready(&self) -> Result<()> {
        for snapshot in self.tasks.database.waiting_jobs().await? {
            let Some(wait) = &snapshot.wait else {
                continue;
            };
            let handler = self.registry.handlers.get(&snapshot.kind);
            let callback_ready = if let Some((_, ready)) = handler {
                match wait.resume {
                    ResumeCondition::Ready => ready(snapshot.clone()).await,
                    ResumeCondition::NotBefore { .. } => false,
                    ResumeCondition::ReadyOrNotBefore { unix_ms } => {
                        if self.tasks.clock.utc().timestamp_millis() >= unix_ms {
                            false
                        } else {
                            ready(snapshot.clone()).await
                        }
                    }
                }
            } else {
                false
            };
            let mut tx = self.tasks.operation("task.wake", &snapshot.id).await?;
            let mut current = tx.job_record(&snapshot.id).await?;
            if current.state != JobState::Waiting {
                tx.rollback().await?;
                continue;
            }
            match self.tasks.dependency_status_in(&mut tx, &current).await? {
                DependencyStatus::Failed(error) => {
                    self.tasks
                        .change_in(&mut tx, &mut current, JobState::Failed, None, Some(error))
                        .await?;
                    tx.commit(json!({"state":current.state})).await?;
                    continue;
                }
                DependencyStatus::Pending => {
                    tx.rollback().await?;
                    continue;
                }
                DependencyStatus::Ready => {}
            }
            if handler.is_none()
                || current.wait != snapshot.wait
                || current.dependencies != snapshot.dependencies
                || current.dependency_policy != snapshot.dependency_policy
                || current.checkpoint != snapshot.checkpoint
                || current.result != snapshot.result
                || current.updated_at != snapshot.updated_at
            {
                tx.rollback().await?;
                continue;
            }
            let now = self.tasks.clock.utc().timestamp_millis();
            let resumed = match current
                .wait
                .as_ref()
                .expect("waiting task has a wait condition")
                .resume
            {
                ResumeCondition::Ready => callback_ready,
                ResumeCondition::NotBefore { unix_ms } => now >= unix_ms,
                ResumeCondition::ReadyOrNotBefore { unix_ms } => now >= unix_ms || callback_ready,
            };
            if !resumed {
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
                // The backend's post-ack commit notification retries this persisted wait.
                Err(error) if error.code == ErrorCode::Conflict => tx.rollback().await?,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    async fn execute(&self, id: &str) -> Result<()> {
        let job = self.tasks.database.job_record(id).await?;
        let span = tracing::info_span!(
            "task.execute", trace_id=%job.trace_id, job_id=%job.id, job_kind=%job.kind,
            namespace_id=job.namespace_id.as_ref().map(|id|id.as_str()).unwrap_or(""),
            subject_id=job.subject_id.as_ref().map(|id|id.as_str()).unwrap_or(""),
        );
        crate::observability::in_trace(job.trace_id.clone(), async {
            let start = Instant::now();
            let result = self.execute_job(id).await;
            let state = self.tasks.database.job_record(id).await.ok()
                .and_then(|job|serde_json::to_value(job.state).ok())
                .and_then(|state|state.as_str().map(str::to_owned))
                .unwrap_or_else(||"unknown".into());
            let elapsed = start.elapsed();
            metrics::counter!("nous_tasks_total", "kind"=>job.kind.clone(), "state"=>state.clone()).increment(1);
            metrics::histogram!("nous_task_duration_seconds", "kind"=>job.kind.clone()).record(elapsed.as_secs_f64());
            match &result {
                Ok(()) => tracing::info!(state,elapsed_ms=elapsed.as_millis() as u64,"task dispatch completed"),
                Err(error) => tracing::warn!(%error,state,elapsed_ms=elapsed.as_millis() as u64,"task dispatch failed"),
            }
            result
        }).instrument(span).await
    }

    async fn execute_job(&self, id: &str) -> Result<()> {
        let mut tx = self.tasks.operation("task.start", id).await?;
        let mut job = tx.job_record(id).await?;
        if terminal(job.state) || job.state == JobState::Waiting {
            tx.rollback().await?;
            return Ok(());
        }
        match self.tasks.dependency_status_in(&mut tx, &job).await? {
            DependencyStatus::Ready => {}
            DependencyStatus::Pending => {
                self.tasks
                    .change_in(
                        &mut tx,
                        &mut job,
                        JobState::Waiting,
                        Some(dependency_wait()),
                        None,
                    )
                    .await?;
                tx.commit(json!({"state":job.state})).await?;
                return Ok(());
            }
            DependencyStatus::Failed(error) => {
                self.tasks
                    .change_in(&mut tx, &mut job, JobState::Failed, None, Some(error))
                    .await?;
                tx.commit(json!({"state":job.state})).await?;
                return Ok(());
            }
        }
        let Some((handler, _)) = self.registry.handlers.get(&job.kind) else {
            let error = Error::new(
                ErrorCode::CapabilityUnavailable,
                "task handler is unavailable",
            );
            let wait = WaitCondition {
                kind: "capability".into(),
                requires_attention: true,
                resume: ResumeCondition::Ready,
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
            let attempts = self
                .tasks
                .database
                .queue_record(id)
                .await?
                .ok_or_else(|| Error::internal("running task has no queue record"))?
                .attempts;
            let mut tx = self.tasks.operation("task.failure", id).await?;
            let mut job = tx.job_record(id).await?;
            if terminal(job.state) || job.state == JobState::Waiting {
                tx.rollback().await?;
                return Ok(());
            }
            let retry = matches!(
                error.code,
                ErrorCode::Internal | ErrorCode::DependencyUnavailable
            ) && attempts < i64::from(self.tasks.config.max_queue_attempts);
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
        self.tasks.notify_wake();
        Ok(())
    }
}
async fn dispatch(job: nous_storage::JobEnvelope, runtime: Data<TaskRuntime>) -> Result<()> {
    runtime.execute(&job.id).await
}
