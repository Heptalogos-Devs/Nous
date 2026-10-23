//! Model resources, frozen invocation inputs, and provider execution.
//! ModelCalls owns durable reservations; domain handlers own adoption.

mod error;
mod execution;
mod formation;

pub mod embedding;
mod resource;
pub mod structured;

pub use crate::common::prompts::{
    PromptArgument, PromptArguments, PromptBinding, PromptSnapshot, PromptTemplate, Prompts,
};
pub use embedding::{
    EmbeddingCallMetadata, EmbeddingCallRecord, EmbeddingConfig, EmbeddingExecution,
    PreparedEmbedding,
};
pub use error::{ModelFailure, ModelFailureInfo, RetryClass};
pub use execution::{CallMetadata, CallReservation, ModelCallRecord, ModelExecution, PreparedCall};
pub use formation::{
    FormationCallRecord, FormationConfig, FormationExecution, FormationInput, ModelCallMetadata,
    PreparedFormation,
};
pub use resource::{
    ModelCapability, ModelConfig, ModelModality, ModelProtocol, ModelReadiness,
    ModelReadinessState, ModelRequirements, ModelResource, ModelResources, ResolvedModel,
    ResourceMetadata,
};
pub use structured::{
    PreparedStructured, StructuredCallMetadata, StructuredCallRecord, StructuredExecution,
    StructuredFormationConfig, StructuredFormationInput,
};

use crate::common::instance::InstancePaths;
use anyhow::Result;
use execution::CallExecutor;
use nous_domain::ModelRole;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct ModelRuntime {
    config: ModelConfig,
    resources: ModelResources,
    prompts: Prompts,
    executor: CallExecutor,
}

impl ModelRuntime {
    pub fn new(paths: &InstancePaths, config: &ModelConfig, http: reqwest::Client) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config: config.clone(),
            resources: ModelResources::new(paths, config)?,
            prompts: Prompts::new(paths.clone()),
            executor: CallExecutor::new(http)?,
        })
    }

    pub fn resources(&self) -> &ModelResources {
        &self.resources
    }
    pub fn ready(&self, role: ModelRole) -> bool {
        self.resources.readiness(role).is_ready()
    }
    /// Local diagnostics use the same resource and prompt resolvers as prepare.
    pub fn inspect(&self) -> Value {
        json!({"memory_formation":self.memory_inspect(),
            "structured_formation":self.structured_inspect(),
            "text_embedding":self.embedding_inspect()})
    }

    pub fn prompts(&self) -> &Prompts {
        &self.prompts
    }

    /// Preview the exact current prompt. Each prepared call retains its snapshot.
    pub fn preview_prompt(
        &self,
        role: ModelRole,
        arguments: &PromptArguments,
    ) -> Result<PromptSnapshot, ModelFailure> {
        let binding = match role {
            ModelRole::MemoryFormation => self.config.memory_formation.as_ref().map(|c| &c.prompt),
            ModelRole::StructuredFormation => {
                self.config.structured_formation.as_ref().map(|c| &c.prompt)
            }
            ModelRole::TextEmbedding => None,
        }
        .ok_or_else(|| ModelFailure::waiting("a prompt-bearing role is not configured"))?;
        self.prompts
            .preview(
                match role {
                    ModelRole::MemoryFormation => "memory_formation",
                    ModelRole::StructuredFormation => "structured_formation",
                    ModelRole::TextEmbedding => unreachable!(),
                },
                binding,
                arguments,
            )
            .map_err(ModelFailure::prompt)
    }
}
