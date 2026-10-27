use nous_domain::*;
use nous_foundation::{configuration::ConfigKey, models::*};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ROLE: &str = "query_rerank";
const BINDING: ConfigKey<Option<RoleBinding>> = ConfigKey::new("models.roles.query_rerank");

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RerankInput {
    pub query: String,
    pub documents: Vec<RerankDocument>,
    pub top_n: usize,
    pub query_sources: Vec<SourceReadRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RerankDocument {
    pub reference: RetrievalTarget,
    pub text: String,
    pub original_bytes: usize,
    pub current_head: bool,
}
pub fn definition() -> RoleDefinition {
    RoleDefinition {
        id: ROLE,
        binding_key: BINDING,
        requirements: ModelRequirements {
            input_modalities: vec![ModelModality::Text],
            capabilities: vec![ModelCapability::Rerank],
        },
        prompt: None,
        contract: RoleContract::new::<RerankInput>(|input| {
            let mut requirements = input.query_sources;
            for document in &input.documents {
                document.reference.validate()?;
                requirements.push(SourceReadRef::target(&document.reference));
            }
            requirements.sort();
            requirements.dedup();
            Ok(InvocationDraft::rerank(
                input.query,
                input
                    .documents
                    .into_iter()
                    .map(|document| document.text)
                    .collect(),
                input.top_n,
            )
            .with_sources(requirements))
        }),
    }
}
pub type Metadata = InvocationMetadata<RerankInput>;
pub async fn execute(
    runtime: &ModelRuntime,
    prepared: PreparedCall<Metadata>,
) -> ModelExecution<Metadata, Vec<RerankScore>> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| match output {
            ModelOutput::Reranked(scores) => Ok(scores),
            _ => Err(ModelFailure::domain(Error::internal(
                "rerank requires indexed relevance scores",
            ))),
        })
}
