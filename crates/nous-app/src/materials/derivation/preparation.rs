use crate::App;
use nous_domain::{media::*, *};
use serde_json::json;
use tokio_util::sync::CancellationToken;

impl App {
    pub(super) async fn prepare_media_run(
        &self,
        run: MediaDerivationRun,
        cancel: &CancellationToken,
    ) -> Result<MediaDerivationRun> {
        if run.prepared.is_some() {
            return Ok(run);
        }
        let (namespace, subject) = run.job.subject_scope()?;
        let context = run.job.call_context("nous-media")?;
        let graph = self
            .prepare_material_source(
                &context,
                namespace,
                subject,
                &run.request.source,
                run.request.delivery,
                cancel,
            )
            .await?;
        let maximum = if run.request.strategy == DerivationStrategy::DescribeThenStructure {
            63
        } else {
            64
        };
        if graph.inputs.len() > maximum {
            return Err(Error::invalid(
                "media preparation exceeds the invocation's input catalog capacity",
            ));
        }
        let Some(mut tx) = self
            .worker_transaction(
                &run.job,
                "media.run_prepared",
                &run.job.id,
                &run.job.id,
                &json!({"job_id":run.job.id}),
            )
            .await?
        else {
            return self.database.media_derivation(&run.job.id).await;
        };
        let current = tx.media_derivation(&run.job.id).await?;
        if current.job.state == JobState::Cancelled || current.prepared.is_some() {
            tx.rollback().await?;
            return Ok(current);
        }
        tx.set_media_prepared(&run.job.id, &graph).await?;
        tx.commit(json!({"prepared":graph})).await?;
        self.database.media_derivation(&run.job.id).await
    }
}
