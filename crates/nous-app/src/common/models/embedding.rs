//! Explicit OpenAI batch embeddings adapted from genai 0.7.0's embedding types.
//! MIT OR Apache-2.0, commit 8fc13af4db80acd0771c87f6180d79b83c9a898d.
use super::{
    ModelCallRecord, ModelExecution, ModelFailure, ModelProtocol, ModelResource, ModelRuntime,
    PreparedCall, ResourceMetadata, RetryClass,
    execution::{CallMetadata, PreparedRequest, ProviderOutput},
    resource::validate_resource,
};
use anyhow::{Result as ConfigResult, bail};
use genai::embed::{EmbedOptions, EmbedRequest, EmbedResponse};
use nous_domain::{EmbeddingInputItem, EmbeddingSpace, ErrorCode, ModelRole};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
    pub resource: ModelResource,
    pub expected_provider_model: String,
    pub dimension: usize,
    #[serde(default = "batch_limit")]
    pub max_batch_size: usize,
    #[serde(default = "input_limit")]
    pub max_input_bytes: usize,
    #[serde(default = "daily_limit")]
    pub max_calls_per_day: u32,
}
fn batch_limit() -> usize {
    32
}
fn input_limit() -> usize {
    32 * 1024
}
fn daily_limit() -> u32 {
    100
}
impl EmbeddingConfig {
    pub fn validate(&self) -> ConfigResult<()> {
        validate_resource(&self.resource)?;
        if self.resource.protocol != ModelProtocol::OpenAiEmbeddings {
            bail!("text embedding requires openai_embeddings protocol");
        }
        if self.expected_provider_model.trim().is_empty()
            || self.expected_provider_model.len() > 255
            || self.expected_provider_model.chars().any(char::is_control)
        {
            bail!("embedding expected_provider_model must contain 1 to 255 visible bytes");
        }
        if !(1..=65536).contains(&self.dimension) {
            bail!("embedding dimension must be between 1 and 65536");
        }
        if !(1..=256).contains(&self.max_batch_size) {
            bail!("embedding max_batch_size must be between 1 and 256");
        }
        if !(1..=1024 * 1024).contains(&self.max_input_bytes) || self.max_calls_per_day > 100000 {
            bail!("embedding input bytes must be 1 to 1048576 and daily calls at most 100000");
        }
        Ok(())
    }
    /// Vector meaning binds actual resource/model/dimension; budgets do not change it.
    pub fn space(&self) -> EmbeddingSpace {
        let material = serde_json::to_vec(&(
            "text",
            ResourceMetadata::from(&self.resource),
            &self.expected_provider_model,
            self.dimension,
        ))
        .expect("space inputs are serializable");
        let digest = format!("{:x}", Sha256::digest(material));
        EmbeddingSpace {
            id: format!("embspace_{digest}"),
            resource_id: self.resource.id.clone(),
            provider_model: self.expected_provider_model.clone(),
            dimension: self.dimension,
            producer_signature: format!("embproducer_{digest}"),
            modality: "text".into(),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingCallMetadata {
    pub call_id: String,
    pub role: ModelRole,
    pub resource: ResourceMetadata,
    pub space: EmbeddingSpace,
    pub producer_signature: String,
    pub inputs: Vec<EmbeddingInputItem>,
    pub parameters: Value,
    pub max_calls_per_day: u32,
}
impl CallMetadata for EmbeddingCallMetadata {
    fn reservation_fields(&self) -> (&str, &str, u32) {
        (&self.call_id, &self.resource.id, self.max_calls_per_day)
    }
}
pub type PreparedEmbedding = PreparedCall<EmbeddingCallMetadata>;
pub type EmbeddingCallRecord = ModelCallRecord<EmbeddingCallMetadata>;
pub type EmbeddingExecution = ModelExecution<EmbeddingCallMetadata, Vec<Vec<f32>>>;
impl ModelRuntime {
    pub fn embedding_config(&self) -> Option<&EmbeddingConfig> {
        self.config.text_embedding.as_ref()
    }
    pub fn embedding_inspect(&self) -> Value {
        let Some(config) = self.embedding_config() else {
            return json!({"state":"waiting_configuration","reason":"text embedding resource is not configured"});
        };
        let mut value = self.resources.inspect(&config.resource);
        value["space"] = json!(config.space());
        value["expected_provider_model"] = json!(config.expected_provider_model);
        value["dimension"] = json!(config.dimension);
        value["max_batch_size"] = json!(config.max_batch_size);
        value["max_input_bytes"] = json!(config.max_input_bytes);
        value["max_calls_per_day"] = json!(config.max_calls_per_day);
        value["cost"] = Value::Null;
        value
    }
    pub fn prepare_embedding(
        &self,
        inputs: Vec<EmbeddingInputItem>,
    ) -> Result<PreparedEmbedding, ModelFailure> {
        let config = self
            .embedding_config()
            .ok_or_else(|| ModelFailure::waiting("text embedding resource is not configured"))?;
        if inputs.is_empty() || inputs.len() > config.max_batch_size {
            return Err(invalid("embedding input batch must fit max_batch_size"));
        }
        let mut bytes = 0usize;
        let mut keys = BTreeSet::new();
        for item in &inputs {
            item.validate().map_err(ModelFailure::domain)?;
            bytes = bytes
                .checked_add(item.text.len())
                .ok_or_else(|| exhausted("embedding input size overflow"))?;
            if bytes > config.max_input_bytes {
                return Err(exhausted("embedding source exceeds max_input_bytes"));
            }
            if !keys.insert(item.source_key().map_err(ModelFailure::domain)?) {
                return Err(invalid("embedding input targets must be unique"));
            }
        }
        let request =
            EmbedRequest::new_batch(inputs.iter().map(|item| item.text.clone()).collect());
        let space = config.space();
        let target = self
            .resources
            .resolve(
                nous_domain::ModelRole::TextEmbedding,
                &super::ModelRequirements::text_embedding(),
            )?
            .into_target();
        Ok(PreparedCall {
            metadata: EmbeddingCallMetadata {
                call_id: uuid::Uuid::new_v4().to_string(),
                role: ModelRole::TextEmbedding,
                resource: (&config.resource).into(),
                producer_signature: space.producer_signature.clone(),
                space,
                inputs,
                parameters: json!({"encoding_format":"float","max_batch_size":config.max_batch_size,"max_input_bytes":config.max_input_bytes}),
                max_calls_per_day: config.max_calls_per_day,
            },
            target,
            request: PreparedRequest::Embedding {
                request,
                options: EmbedOptions::new()
                    .with_encoding_format("float")
                    .with_capture_raw_body(true),
            },
        })
    }
    pub async fn execute_embedding(&self, prepared: PreparedEmbedding) -> EmbeddingExecution {
        self.executor
            .invoke(prepared)
            .await
            .map(|output, metadata| match output {
                ProviderOutput::Embeddings(response) => validate_response(*response, metadata),
                _ => Err(ModelFailure::new(
                    RetryClass::Permanent,
                    ErrorCode::Internal,
                    "embedding requires vector output",
                )),
            })
    }
}
fn invalid(message: &'static str) -> ModelFailure {
    ModelFailure::new(RetryClass::Permanent, ErrorCode::InvalidArgument, message)
}
fn exhausted(message: &'static str) -> ModelFailure {
    ModelFailure::new(RetryClass::Permanent, ErrorCode::ResourceExhausted, message)
}
fn validate_response(
    response: EmbedResponse,
    metadata: &EmbeddingCallMetadata,
) -> Result<Vec<Vec<f32>>, ModelFailure> {
    if response.provider_model_iden.model_name != metadata.space.provider_model {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::Conflict,
            "embedding provider model differs from the configured immutable space",
        ));
    }
    if response.embeddings.len() != metadata.inputs.len() {
        return Err(invalid(
            "embedding response must cover every input exactly once",
        ));
    }
    let mut ordered = vec![None; metadata.inputs.len()];
    for embedding in response.embeddings {
        if embedding.index >= ordered.len() || ordered[embedding.index].is_some() {
            return Err(invalid(
                "embedding response contains a duplicate or out-of-range index",
            ));
        }
        let vector = embedding.vector;
        if vector.len() != metadata.space.dimension
            || !vector.iter().all(|value| value.is_finite())
            || !vector.iter().any(|value| *value != 0.0)
        {
            return Err(invalid(
                "embedding response must contain finite nonzero vectors with the configured dimension",
            ));
        }
        ordered[embedding.index] = Some(vector);
    }
    ordered
        .into_iter()
        .map(|vector| vector.ok_or_else(|| invalid("embedding response is missing an input index")))
        .collect()
}
