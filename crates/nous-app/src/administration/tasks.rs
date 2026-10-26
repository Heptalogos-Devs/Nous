use crate::App;
use nous_domain::{CallContext, JobRecord, Result};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl App {
    pub async fn wait_jobs(
        &self,
        ctx: &CallContext,
        ids: &[String],
        timeout_ms: u64,
        cancel: &CancellationToken,
    ) -> Result<Vec<JobRecord>> {
        self.tasks.wait_all(ctx, ids, Duration::from_millis(timeout_ms), cancel).await
    }
}
