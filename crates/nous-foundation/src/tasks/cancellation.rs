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
        job.require_read(tx.context())?;
        if !terminal(job.state) {
            self.change_in(tx, job, JobState::Cancelled, None, error)
                .await?;
        }
        if job.state == JobState::Cancelled {
            tx.retain_until_end(CancelAfterEnd {
                tasks: self.clone(),
                id: job.id.clone(),
                trace: tx.context().trace_id.clone(),
                span: tracing::Span::current(),
            });
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
