mod graph;
mod ingest;
mod maintain;
mod policy;
mod profile;
mod search;
mod time;
mod types;
use crate::App;
use nous_domain::*;
pub use policy::register_configuration;
use serde_json::Value;
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
        match &request.operation {
            AmemorixOperation::SearchMemory(input) => {
                search::execute(self, ctx, &request, input, &policy).await
            }
            AmemorixOperation::IngestText(input) => {
                ingest::execute(self, ctx, &request, input, &policy).await
            }
            AmemorixOperation::IngestSummary(input) => {
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
                ingest::execute(self, ctx, &request, &input, &policy).await
            }
            AmemorixOperation::MaintainMemory(input) => {
                maintain::execute(self, ctx, &request, input).await
            }
            AmemorixOperation::GetPersonProfile(input) => {
                profile::execute(self, ctx, &request, input).await
            }
            AmemorixOperation::MemorySource(input) => {
                self.retrieval_source(ctx, &request.subject_id, &input.target)
                    .await
            }
        }
    }
}
