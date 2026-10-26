mod admission;
mod catalog;
mod compile;
pub(crate) mod config;
mod feedback;
pub(crate) mod model;
mod preparation;
mod read;
mod residents;
mod reuse;
mod sessions;
mod work;
mod worker;
use crate::App;
use nous_domain::{context_work::*, *};
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use serde_json::json;

fn scope(ns: &NamespaceId, subject: &SubjectId) -> String {
    format!("namespace:{ns}/subject:{subject}")
}
fn accepted_context(ctx: &CallContext, constraints: &[SourceReadScope]) -> Result<CallContext> {
    constraints.iter().try_fold(ctx.clone(), |context, scope| {
        context.with_source_read(scope.clone())
    })
}
impl App {
    pub async fn context_preparation(
        &self,
        ctx: &CallContext,
        id: &ContextPreparationId,
    ) -> Result<ContextPreparation> {
        let value = self.database.context_preparation(id).await?;
        self.tasks.read(ctx, &value.job.id).await?;
        let (ns, subject) = value.job.subject_scope()?;
        let context = accepted_context(ctx, &value.job.source_read)?;
        let ctx = &context;
        ctx.require_subject(ns, subject)?;
        let mut read = self.database.read_view().await?;
        for target in value.catalog.iter().flat_map(|segment| &segment.sources) {
            read.require_source_read(ctx, ns, subject, target).await?;
        }
        if let Some(context) = &value.work_context {
            read.require_work_context_read(ctx, ns, subject, &context.id)
                .await?;
        }
        read.finish().await?;
        Ok(value)
    }
    pub async fn context_admission(
        &self,
        ctx: &CallContext,
        id: &ContextAdmissionId,
    ) -> Result<ContextAdmission> {
        let value = self.database.context_admission(id).await?;
        let projection = self
            .database
            .context_projection(&value.projection_id)
            .await?;
        let context = accepted_context(ctx, &projection.source_read)?;
        let ctx = &context;
        ctx.require_subject(&value.namespace_id, &value.subject_id)?;
        let mut read = self.database.read_view().await?;
        for target in &value.sources {
            read.require_source_read(ctx, &value.namespace_id, &value.subject_id, target)
                .await?;
        }
        read.finish().await?;
        Ok(value)
    }
}
