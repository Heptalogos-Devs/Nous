mod bundles;
mod diagnostics;
mod graph;
mod imports;
mod ingest;
mod maintain;
mod policy;
mod profile;
mod search;
mod sources;
mod time;
mod types;
use crate::App;
pub use bundles::BundleInput;
use futures::future::BoxFuture;
pub use imports::ImportInput;
use nous_domain::*;
pub(crate) use policy::AMEMORIX;
pub use policy::register_configuration;
use serde_json::Value;
pub use sources::{DeleteAdminInput, SourceAdminInput};
pub use types::*;

impl App {
    pub async fn amemorix(&self, ctx: &CallContext, request: AmemorixRequest) -> Result<Value> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        self.database
            .subject(&request.namespace_id, &request.subject_id)
            .await?;
        let policy = self
            .configuration
            .snapshot(
                &ConfigScope::Subject {
                    namespace_id: request.namespace_id.clone(),
                    subject_id: request.subject_id.clone(),
                },
                false,
            )
            .await?
            .get(policy::AMEMORIX)?;
        self.amemorix_operation(ctx, &request, &policy).await
    }

    fn amemorix_operation<'a>(
        &'a self,
        ctx: &'a CallContext,
        request: &'a AmemorixRequest,
        policy: &'a policy::AmemorixPolicy,
    ) -> BoxFuture<'a, Result<Value>> {
        match &request.operation {
            AmemorixOperation::MemorySourceAdmin(input) => {
                Box::pin(sources::execute(self, ctx, request, input))
            }
            AmemorixOperation::MemoryDeleteAdmin(input) => {
                Box::pin(sources::delete_admin(self, ctx, request, input))
            }
            AmemorixOperation::MemoryImportAdmin(input) => {
                imports::execute(self, ctx, request, input)
            }
            AmemorixOperation::MemoryBundleAdmin(input) => {
                Box::pin(bundles::execute(self, ctx, request, input))
            }
            AmemorixOperation::MemoryStats(_) => {
                Box::pin(diagnostics::stats(self, ctx, &request.subject_id))
            }
            AmemorixOperation::MemoryRuntimeAdmin(input) => {
                Box::pin(diagnostics::runtime(self, ctx, &request.subject_id, input))
            }
            AmemorixOperation::SearchMemory(input) => {
                Box::pin(search::execute(self, ctx, request, input, policy))
            }
            AmemorixOperation::IngestText(input) => {
                Box::pin(ingest::execute(self, ctx, request, input, policy))
            }
            AmemorixOperation::IngestSummary(input) => Box::pin(async move {
                let input = IngestInput {
                    external_id: input.external_id.clone(),
                    source_type: "chat_summary".into(),
                    text: input.text.clone(),
                    chat_id: input.chat_id.clone(),
                    person_ids: vec![],
                    participants: input.participants.clone(),
                    timestamp: None,
                    time_start: input.time_start.clone(),
                    time_end: input.time_end.clone(),
                    tags: input.tags.clone(),
                    metadata: input.metadata.clone(),
                    entities: vec![],
                    relations: vec![],
                    respect_filter: input.respect_filter,
                    user_id: input.user_id.clone(),
                    group_id: input.group_id.clone(),
                };
                ingest::execute(self, ctx, request, &input, policy).await
            }),
            AmemorixOperation::MaintainMemory(input) => {
                Box::pin(maintain::execute(self, ctx, request, input))
            }
            AmemorixOperation::GetPersonProfile(input) => {
                Box::pin(profile::execute(self, ctx, request, input))
            }
            AmemorixOperation::MemorySource(input) => {
                Box::pin(self.retrieval_source(ctx, &request.subject_id, &input.target))
            }
        }
    }
}
