//! Consumers register their typed input contract and pure message/output builder.
use super::{ModelInput, ModelMessage, OutputContract};
use crate::prompts::PromptArguments;
use nous_domain::{Error, Result};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvocationDraft {
    Chat {
        sources: Option<Vec<nous_domain::SourceReadRef>>,
        arguments: PromptArguments,
        messages: Vec<ModelMessage>,
        output: OutputContract,
    },
    Embedding {
        sources: Option<Vec<nous_domain::SourceReadRef>>,
        inputs: Vec<Vec<nous_domain::EmbeddingPart>>,
    },
}
impl InvocationDraft {
    pub fn chat(
        arguments: PromptArguments,
        messages: Vec<ModelMessage>,
        output: OutputContract,
    ) -> Self {
        Self::Chat {
            sources: None,
            arguments,
            messages,
            output,
        }
    }
    pub fn embedding(inputs: Vec<Vec<nous_domain::EmbeddingPart>>) -> Self {
        Self::Embedding {
            inputs,
            sources: None,
        }
    }
    pub fn with_sources(mut self, requirements: Vec<nous_domain::SourceReadRef>) -> Self {
        match &mut self {
            Self::Chat { sources, .. } | Self::Embedding { sources, .. } => {
                *sources = Some(requirements)
            }
        }
        self
    }
    pub(super) fn input(
        self,
    ) -> (
        PromptArguments,
        ModelInput,
        Option<Vec<nous_domain::SourceReadRef>>,
    ) {
        match self {
            Self::Chat {
                sources,
                arguments,
                messages,
                output,
            } => (arguments, ModelInput::Chat { messages, output }, sources),
            Self::Embedding { inputs, sources } => (
                PromptArguments::new(),
                ModelInput::Embedding { inputs },
                sources,
            ),
        }
    }
}
type Builder = Arc<dyn Fn(&Value) -> Result<InvocationDraft> + Send + Sync>;
#[derive(Clone)]
pub struct RoleContract {
    pub input_schema: Value,
    build: Builder,
}
impl RoleContract {
    pub fn new<T: DeserializeOwned + JsonSchema + 'static>(
        build: impl Fn(T) -> Result<InvocationDraft> + Send + Sync + 'static,
    ) -> Self {
        Self {
            input_schema: serde_json::to_value(schemars::schema_for!(T))
                .expect("input schema serializable"),
            build: Arc::new(move |value| {
                let typed = serde_json::from_value(value.clone()).map_err(|_| {
                    Error::invalid("input does not match the registered role contract")
                })?;
                build(typed)
            }),
        }
    }
    pub(super) fn build(&self, value: &Value) -> Result<InvocationDraft> {
        (self.build)(value)
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct RoleContractView {
    pub role: String,
    pub input_schema: Value,
    pub requirements: super::ModelRequirements,
    pub prompt_id: Option<String>,
    pub binding: Option<super::RoleBinding>,
    pub binding_source: Option<crate::configuration::ConfigSource>,
    pub prompt: Option<crate::prompts::PromptTemplate>,
    pub prompt_error: Option<Error>,
    pub draft: Option<InvocationDraft>,
}
