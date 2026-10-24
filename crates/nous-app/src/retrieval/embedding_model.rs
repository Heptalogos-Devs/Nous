//! The indexing consumer owns CognitiveRef projection inputs and declares its role.
use crate::common::{
    configuration::ConfigKey,
    models::{
        AttemptContext, InvocationDraft, InvocationMetadata, InvocationPlan, ModelExecution,
        ModelFailure, ModelOutput, ModelRequirements, ModelRuntime, PreparedCall, RoleBinding,
        RoleContract, RoleDefinition,
    },
};
use nous_domain::{EmbeddingInputItem, EmbeddingSpace, Error};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const ROLE: &str = "text_embedding";
pub const BINDING: ConfigKey<Option<RoleBinding>> = ConfigKey::new("models.roles.text_embedding");
pub fn definition() -> RoleDefinition {
    RoleDefinition {
        id: ROLE,
        binding_key: BINDING,
        requirements: ModelRequirements::text_embedding(),
        prompt: None,
        contract: RoleContract::new::<EmbeddingInput>(|input| {
            validate_inputs(&input.inputs)?;
            Ok(InvocationDraft::embedding(
                input.inputs.into_iter().map(|item| item.text).collect(),
            ))
        }),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingInput {
    pub space: EmbeddingSpace,
    pub inputs: Vec<EmbeddingInputItem>,
}
pub type PreparedEmbedding = PreparedCall<InvocationMetadata<EmbeddingInput>>;
pub async fn prepare(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    inputs: Vec<EmbeddingInputItem>,
) -> Result<PreparedEmbedding, ModelFailure> {
    let space = plan
        .resolve(&attempt)
        .await?
        .route(attempt.index as usize)
        .space()
        .map_err(ModelFailure::domain)?;
    plan.prepare(
        attempt,
        EmbeddingInput { space, inputs },
        Default::default(),
    )
    .await
}
fn validate_inputs(inputs: &[EmbeddingInputItem]) -> nous_domain::Result<()> {
    let mut keys = BTreeSet::new();
    for input in inputs {
        input.validate()?;
        if !keys.insert(input.source_key()?) {
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
