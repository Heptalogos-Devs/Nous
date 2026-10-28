//! Task cancellation becomes effective only after its authority transaction finishes.
use super::{Tasks, terminal};
use crate::{execution, observability::in_trace};
use nous_domain::*;
use nous_storage::{BeginOperation, OperationReceipt, WriteTransaction, operation_key};
use serde_json::json;
use tracing::Instrument;
impl Tasks {
    pub async fn cancel_in(
        &self,
        tx: &mut WriteTransaction,
        job: &mut JobRecord,
        error: Option<Error>,
    ) -> Result<()> {
        let root = tx.job_record(&job.id).await?;
        root.require_read(tx.context())?;
        for id in tx.owned_job_ids(&root.id).await? {
            let mut current = tx.job_record(&id).await?;
            if current.namespace_id != root.namespace_id || current.subject_id != root.subject_id {
                return Err(Error::internal("owned task tree leaves its parent scope"));
            }
            if !terminal(current.state) {
                let reason = error.clone().or_else(|| {
                    if id != root.id {
                        Some(
                            Error::new(
                                ErrorCode::Cancelled,
                                "owning task cancellation was requested",
                            )
                            .with_context("parent_job_id", root.id.clone()),
                        )
                    } else {
                        None
                    }
                });
                self.change_in(tx, &mut current, JobState::Cancelled, None, reason)
                    .await?;
            }
            if current.state == JobState::Cancelled {
                tx.retain_until_end(CancelAfterEnd {
                    tasks: self.clone(),
                    id: current.id.clone(),
                    trace: tx.context().trace_id.clone(),
                    span: tracing::Span::current(),
                });
            }
            if id == root.id {
                *job = current;
            }
        }
        Ok(())
    }
    pub async fn cancel(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &str,
    ) -> Result<OperationReceipt> {
        self.read(ctx, id).await?;
        let key = operation_key(ctx, "task.cancel", id, operation_id, &json!({"job_id":id}))?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut job = tx.job_record(id).await?;
        self.cancel_in(&mut tx, &mut job, None).await?;
        tx.commit(json!(job)).await
    }
}
struct CancelAfterEnd {
    tasks: Tasks,
    id: String,
    trace: TraceId,
    span: tracing::Span,
}
impl std::fmt::Debug for CancelAfterEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelAfterEnd")
            .field("job_id", &self.id)
            .finish()
    }
}
impl Drop for CancelAfterEnd {
    fn drop(&mut self) {
        let tasks = self.tasks.clone();
        let id = self.id.clone();
        let trace = self.trace.clone();
        execution::cleanup(
            in_trace(trace, async move {
                if tasks.database.job_record(&id).await?.state == JobState::Cancelled
                    && let Some(token) = tasks.running.lock().expect("task registry lock").get(&id)
                {
                    token.cancel();
                }
                Ok(())
            })
            .instrument(self.span.clone()),
        );
    }
}
