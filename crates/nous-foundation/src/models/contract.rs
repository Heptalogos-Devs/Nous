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
        arguments: PromptArguments,
        messages: Vec<ModelMessage>,
        output: OutputContract,
    },
    Embedding {
        texts: Vec<String>,
    },
}
impl InvocationDraft {
    pub fn chat(
        arguments: PromptArguments,
        messages: Vec<ModelMessage>,
        output: OutputContract,
    ) -> Self {
        Self::Chat {
            arguments,
            messages,
            output,
        }
    }
    pub fn embedding(texts: Vec<String>) -> Self {
        Self::Embedding { texts }
    }
    pub(super) fn input(self) -> (PromptArguments, ModelInput) {
        match self {
            Self::Chat {
                arguments,
                messages,
                output,
            } => (arguments, ModelInput::Chat { messages, output }),
            Self::Embedding { texts } => (PromptArguments::new(), ModelInput::Embedding { texts }),
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
