use super::{Tasks, authorize, terminal};
use nous_domain::{CallContext, Error, ErrorCode, JobRecord, JobState, Result};
use nous_storage::WriteTransaction;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

impl Tasks {
    pub async fn wait_all(
        &self,
        ctx: &CallContext,
        ids: &[String],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Vec<JobRecord>> {
        let cancelled = || {
            Error::new(ErrorCode::Cancelled, "task waiting cancelled")
                .with_context("job_ids", json!(ids))
                .with_trace(ctx.trace_id.clone())
        };
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        for id in ids {
            nous_domain::validate_name(id, "job_id")?;
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::invalid("task waiting timeout exceeds the clock range"))?;
        let waiting = async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let jobs = self.wait_snapshot(ctx, ids).await?;
                if jobs.iter().all(|job| terminal(job.state))
                    || jobs.iter().any(|job| {
                        job.state == JobState::Waiting
                            && job
                                .wait
                                .as_ref()
                                .is_some_and(|wait| wait.requires_attention)
                    })
                {
                    return Ok(jobs);
                }
                tokio::select! {
                    _ = changed => {},
                    _ = tokio::time::sleep(Duration::from_millis(self.config.poll_interval_ms)) => {},
                }
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(cancelled()),
            result = tokio::time::timeout_at(deadline, waiting) => match result {
                Ok(result) => result,
                Err(elapsed) => Err(Error::new(ErrorCode::DependencyUnavailable, "task waiting deadline elapsed")
                    .with_context("job_ids", json!(ids)).with_cause(&elapsed).with_trace(ctx.trace_id.clone())),
            },
        }
    }

    async fn wait_snapshot(&self, ctx: &CallContext, ids: &[String]) -> Result<Vec<JobRecord>> {
        let mut view = self.database.read_view().await?;
        let mut jobs = Vec::with_capacity(ids.len());
        for id in ids {
            let mut job = view.job_record(id).await?;
            // Waiting exposes lifecycle facts, while full task bodies use read().
            match (&job.namespace_id, &job.subject_id) {
                (Some(namespace), Some(subject)) => ctx.require_subject(namespace, subject)?,
                (Some(namespace), None) => ctx.require_namespace(namespace)?,
                _ => ctx.require_admin()?,
            }
            if authorize(ctx, &job).is_err() {
                job.checkpoint = None;
                job.result = None;
                if let Some(wait) = &mut job.wait {
                    wait.details = json!({});
                }
                job.error = job.error.map(|error| {
                    Error::new(error.code, "shared task did not complete")
                        .with_trace(job.trace_id.clone())
                });
            }
            jobs.push(job);
        }
        view.finish().await?;
        Ok(jobs)
    }

    pub(super) fn observe_transaction(&self, tx: &mut WriteTransaction) {
        tx.retain_until_end(TaskChange(self.changed.clone()));
    }
}

#[derive(Debug)]
struct TaskChange(Arc<Notify>);
impl Drop for TaskChange {
    fn drop(&mut self) {
        self.0.notify_waiters();
    }
}
