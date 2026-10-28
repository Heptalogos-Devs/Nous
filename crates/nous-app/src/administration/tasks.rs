use crate::App;
use nous_domain::{CallContext, JobListRequest, JobPage, JobRecord, Result};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl App {
    pub async fn jobs(&self, ctx: &CallContext, request: &JobListRequest) -> Result<JobPage> {
        self.tasks.list(ctx, request).await
    }

    pub async fn wait_jobs(
        &self,
        ctx: &CallContext,
        ids: &[String],
        timeout_ms: u64,
        cancel: &CancellationToken,
    ) -> Result<Vec<JobRecord>> {
        self.tasks
            .wait_all(ctx, ids, Duration::from_millis(timeout_ms), cancel)
            .await
    }
}
