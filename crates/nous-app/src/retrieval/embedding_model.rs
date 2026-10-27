//! The indexing consumer owns CognitiveRef projection inputs and declares its role.
use nous_domain::{EmbeddingInputItem, Error};
use nous_foundation::configuration::ConfigKey;
use nous_foundation::models::AttemptContext;
use nous_foundation::models::InvocationDraft;
use nous_foundation::models::InvocationMetadata;
use nous_foundation::models::InvocationPlan;
use nous_foundation::models::ModelExecution;
use nous_foundation::models::ModelFailure;
use nous_foundation::models::ModelOutput;
use nous_foundation::models::ModelRequirements;
use nous_foundation::models::ModelRuntime;
use nous_foundation::models::PreparedCall;
use nous_foundation::models::RoleBinding;
use nous_foundation::models::RoleContract;
use nous_foundation::models::RoleDefinition;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const ROLE: &str = "embedding";
pub const BINDING: ConfigKey<Option<RoleBinding>> = ConfigKey::new("models.roles.embedding");
pub fn definition() -> RoleDefinition {
    RoleDefinition {
        id: ROLE,
        binding_key: BINDING,
        requirements: ModelRequirements::embedding(),
        prompt: None,
        contract: RoleContract::new::<EmbeddingInput>(|input| {
            validate_inputs(&input.inputs)?;
            let mut requirements = Vec::new();
            for item in &input.inputs {
                requirements.extend(item.source_requirements());
            }
            requirements.sort();
            requirements.dedup();
            let draft = InvocationDraft::embedding(
                input.inputs.into_iter().map(|item| item.parts).collect(),
            );
            Ok(draft.with_sources(requirements))
        }),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingInput {
    pub inputs: Vec<EmbeddingInputItem>,
}
pub type PreparedEmbedding = PreparedCall<InvocationMetadata<EmbeddingInput>>;
pub async fn prepare(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    inputs: Vec<EmbeddingInputItem>,
) -> Result<PreparedEmbedding, ModelFailure> {
    plan.prepare(attempt, EmbeddingInput { inputs }, Default::default())
        .await
}
fn validate_inputs(inputs: &[EmbeddingInputItem]) -> nous_domain::Result<()> {
    let mut keys = BTreeSet::new();
    for input in inputs {
        input.validate()?;
        if !keys.insert((input.source_key()?, input.source_hash())) {
            return Err(Error::invalid("embedding targets must be unique"));
        }
    }
    Ok(())
}

pub async fn execute(
    runtime: &ModelRuntime,
    prepared: PreparedEmbedding,
) -> ModelExecution<InvocationMetadata<EmbeddingInput>, Vec<Vec<f32>>> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| match output {
            ModelOutput::Vectors(vectors) => Ok(vectors),
            _ => Err(ModelFailure::domain(Error::internal(
                "embedding requires vector output",
            ))),
        })
}
