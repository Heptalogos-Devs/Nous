use crate::secret::SecretRef;
use nous_domain::{EmbeddingSpace, Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    OpenaiChatCompletions,
    OpenaiEmbeddings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelModality {
    Text,
    Image,
    Audio,
    Video,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    TextGeneration,
    StructuredOutput,
    Embedding,
    Tools,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub endpoint: String,
    pub credential: SecretRef,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default = "daily")]
    pub max_calls_per_day: u32,
}
fn enabled() -> bool {
    true
}
fn daily() -> u32 {
    20
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub provider: String,
    pub protocol: ModelProtocol,
    pub request_model: String,
    pub input_modalities: Vec<ModelModality>,
    pub capabilities: Vec<ModelCapability>,
    pub embedding: Option<EmbeddingProfile>,
    pub pricing: Option<TokenPricing>,
    #[serde(default = "daily")]
    pub max_calls_per_day: u32,
}
pub use nous_domain::TokenPricing;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingProfile {
    pub expected_provider_model: String,
    pub dimension: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RouteParameters {
    pub max_input_bytes: usize,
    pub max_media_bytes: usize,
    pub max_response_bytes: usize,
    pub max_output_tokens: u32,
    pub max_batch_size: usize,
    pub native_parameters: BTreeMap<String, Value>,
}
impl Default for RouteParameters {
    fn default() -> Self {
        Self {
            max_input_bytes: 32 * 1024,
            max_media_bytes: 64 * 1024 * 1024,
            max_response_bytes: 64 * 1024 * 1024,
            max_output_tokens: 1200,
            max_batch_size: 32,
            native_parameters: BTreeMap::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRoute {
    pub model: String,
    #[serde(default)]
    pub parameters: RouteParameters,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCatalog {
    pub providers: BTreeMap<String, Provider>,
    pub models: BTreeMap<String, Model>,
    pub routes: BTreeMap<String, ExecutionRoute>,
}
impl ModelCatalog {
    pub fn validate(&self) -> Result<()> {
        if self.providers.len() > 128 || self.models.len() > 512 || self.routes.len() > 1024 {
            return Err(Error::invalid("model catalog exceeds resource limits"));
        }
        for (id, provider) in &self.providers {
            identifier(id)?;
            nous_domain::validate_daily_call_limit(provider.max_calls_per_day)?;
            provider
                .credential
                .validate()
                .map_err(|error| Error::invalid(error.to_string()))?;
            let url = reqwest::Url::parse(&provider.endpoint)
                .map_err(|_| Error::invalid("provider endpoint must be an absolute HTTP URL"))?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::invalid(
                    "provider endpoint must be HTTP without credentials, query or fragment",
                ));
            }
        }
        for (id, model) in &self.models {
            identifier(id)?;
            nous_domain::validate_daily_call_limit(model.max_calls_per_day)?;
            if !self.providers.contains_key(&model.provider)
                || model.request_model.trim().is_empty()
            {
                return Err(Error::invalid(
                    "model requires an existing provider and request_model",
                ));
            }
            if model.input_modalities.is_empty()
                || model.capabilities.is_empty()
                || (model.protocol == ModelProtocol::OpenaiEmbeddings
                    && model
                        .input_modalities
                        .iter()
                        .any(|m| !matches!(m, ModelModality::Text)))
                || model
                    .capabilities
                    .iter()
                    .any(|c| !adapter_supports(model.protocol, *c))
            {
                return Err(Error::invalid(
                    "model capabilities exceed the installed protocol adapter",
                ));
            }
            match (&model.protocol, &model.embedding) {
                (ModelProtocol::OpenaiEmbeddings, Some(space))
                    if !space.expected_provider_model.trim().is_empty()
                        && (1..=65536).contains(&space.dimension) => {}
                (ModelProtocol::OpenaiChatCompletions, None) => {}
                _ => {
                    return Err(Error::invalid(
                        "embedding protocol requires its actual model and dimension",
                    ));
                }
            }
            if let Some(price) = &model.pricing {
                price.validate()?;
            }
        }
        for (id, route) in &self.routes {
            identifier(id)?;
            if !self.models.contains_key(&route.model) {
                return Err(Error::invalid("route model does not exist"));
            }
            let p = &route.parameters;
            if !(1..=1024 * 1024).contains(&p.max_input_bytes)
                || p.max_media_bytes == 0
                || p.max_media_bytes > 64 * 1024 * 1024
                || p.max_response_bytes == 0
                || p.max_response_bytes > 1024 * 1024 * 1024
                || p.max_output_tokens == 0
                || p.max_output_tokens > 65536
                || !(1..=256).contains(&p.max_batch_size)
            {
                return Err(Error::invalid(
                    "route input/output/batch limits are out of range",
                ));
            }
            if p.native_parameters.len() > 32
                || p.native_parameters.keys().any(|key| {
                    matches!(
                        key.as_str(),
                        "model"
                            | "messages"
                            | "input"
                            | "stream"
                            | "response_format"
                            | "max_tokens"
                    )
                })
            {
                return Err(Error::invalid(
                    "native parameters cannot override invocation identity or contract",
                ));
            }
        }
        Ok(())
    }
}
pub(super) fn identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    {
        return Err(Error::invalid(
            "resource identifier must contain 1..128 ASCII identifier bytes",
        ));
    }
    Ok(())
}
pub(super) fn adapter_supports(protocol: ModelProtocol, capability: ModelCapability) -> bool {
    matches!(
        (protocol, capability),
        (
            ModelProtocol::OpenaiChatCompletions,
            ModelCapability::TextGeneration | ModelCapability::StructuredOutput
        ) | (ModelProtocol::OpenaiEmbeddings, ModelCapability::Embedding)
    )
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMetadata {
    pub id: String,
    pub provider: String,
    pub protocol: ModelProtocol,
    pub endpoint: String,
    pub model: String,
}
pub fn embedding_space(resource: &ResourceMetadata, profile: &EmbeddingProfile) -> EmbeddingSpace {
    let material = serde_json::to_vec(&(
        "text",
        resource,
        &profile.expected_provider_model,
        profile.dimension,
    ))
    .expect("embedding space is serializable");
    let digest = format!("{:x}", Sha256::digest(material));
    EmbeddingSpace {
        id: format!("embspace_{digest}"),
        resource_id: resource.id.clone(),
        provider_model: profile.expected_provider_model.clone(),
        dimension: profile.dimension,
        producer_signature: format!("embproducer_{digest}"),
        modality: "text".into(),
    }
}
