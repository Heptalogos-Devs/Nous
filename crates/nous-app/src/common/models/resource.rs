//! Explicit provider targets adapted from rust-genai c06-target-resolver.rs.
//! genai 0.7.0, MIT OR Apache-2.0, commit 8fc13af4db80acd0771c87f6180d79b83c9a898d.
use super::{
    EmbeddingConfig, FormationConfig, ModelFailure, RetryClass, StructuredFormationConfig,
};
use crate::common::instance::{InstancePaths, SecretRef};
use anyhow::{Result, bail};
use genai::{
    ModelIden, ServiceTarget,
    adapter::AdapterKind,
    resolver::{AuthData, Endpoint},
};
use nous_domain::{ErrorCode, ModelRole};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub memory_formation: Option<FormationConfig>,
    pub text_embedding: Option<EmbeddingConfig>,
    pub structured_formation: Option<StructuredFormationConfig>,
}
impl ModelConfig {
    /// Validation does not access credentials or the network.
    pub fn validate(&self) -> Result<()> {
        if let Some(config) = &self.memory_formation {
            config.validate()?;
        }
        if let Some(config) = &self.text_embedding {
            config.validate()?;
        }
        if let Some(config) = &self.structured_formation {
            config.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelResource {
    pub id: String,
    pub provider: String,
    pub protocol: ModelProtocol,
    pub endpoint: String,
    pub model: String,
    pub credential: SecretRef,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ModelProtocol {
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    #[serde(rename = "openai_embeddings")]
    OpenAiEmbeddings,
}
#[derive(Debug, Clone, Serialize)]
pub struct ResourceMetadata {
    pub id: String,
    pub provider: String,
    pub protocol: ModelProtocol,
    pub endpoint: String,
    pub model: String,
}
impl From<&ModelResource> for ResourceMetadata {
    fn from(value: &ModelResource) -> Self {
        Self {
            id: value.id.clone(),
            provider: value.provider.clone(),
            protocol: value.protocol,
            endpoint: value.endpoint.clone(),
            model: value.model.clone(),
        }
    }
}

pub(super) fn validate_resource(resource: &ModelResource) -> Result<()> {
    for (name, value) in [
        ("resource id", &resource.id),
        ("provider", &resource.provider),
        ("model", &resource.model),
    ] {
        if value.trim().is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
            bail!("model {name} must contain 1 to 255 visible bytes");
        }
    }
    let endpoint = reqwest::Url::parse(&resource.endpoint)?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || !endpoint.path().ends_with('/')
    {
        bail!(
            "model endpoint must be an HTTP base URL ending with '/', without credentials, query or fragment"
        );
    }
    Ok(())
}

pub(super) fn validate_chat(
    resource: &ModelResource,
    input: usize,
    output: u32,
    daily: u32,
    native: &BTreeMap<String, Value>,
) -> Result<()> {
    validate_resource(resource)?;
    if resource.protocol != ModelProtocol::OpenAiChatCompletions {
        bail!("chat role requires openai_chat_completions protocol");
    }
    if !(1..=1024 * 1024).contains(&input) || !(1..=32768).contains(&output) || daily > 100000 {
        bail!(
            "chat input bytes must be 1..1048576, output tokens 1..32768 and daily calls at most 100000"
        );
    }
    const OWNED: &[&str] = &[
        "model",
        "messages",
        "stream",
        "tools",
        "tool_choice",
        "response_format",
        "max_tokens",
        "max_completion_tokens",
    ];
    if native.keys().any(|key| OWNED.contains(&key.as_str())) {
        bail!("native parameters cannot override owned chat request fields");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelModality {
    Text,
    Image,
    Audio,
    Video,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    TextGeneration,
    StructuredOutput,
    Embedding,
    Tools,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequirements {
    pub input_modalities: Vec<ModelModality>,
    pub capabilities: Vec<ModelCapability>,
}
impl ModelRequirements {
    pub fn structured_text() -> Self {
        Self {
            input_modalities: vec![ModelModality::Text],
            capabilities: vec![
                ModelCapability::TextGeneration,
                ModelCapability::StructuredOutput,
            ],
        }
    }
    pub fn text_embedding() -> Self {
        Self {
            input_modalities: vec![ModelModality::Text],
            capabilities: vec![ModelCapability::Embedding],
        }
    }
    fn validate(&self, protocol: ModelProtocol) -> Result<(), ModelFailure> {
        let supported = self
            .input_modalities
            .iter()
            .all(|m| matches!(m, ModelModality::Text))
            && self.capabilities.iter().all(|capability| match capability {
                ModelCapability::Embedding => protocol == ModelProtocol::OpenAiEmbeddings,
                ModelCapability::TextGeneration | ModelCapability::StructuredOutput => {
                    protocol == ModelProtocol::OpenAiChatCompletions
                }
                ModelCapability::Tools => false,
            });
        if !supported {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                ErrorCode::CapabilityUnavailable,
                "the configured model protocol does not support the requested modality or capability",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelReadinessState {
    Ready,
    WaitingConfiguration,
}
#[derive(Debug, Clone, Serialize)]
pub struct ModelReadiness {
    pub role: ModelRole,
    pub state: ModelReadinessState,
    pub resource: Option<ResourceMetadata>,
    pub reason: Option<String>,
}
impl ModelReadiness {
    pub fn is_ready(&self) -> bool {
        self.state == ModelReadinessState::Ready
    }
}

/// Immutable selected route; authentication and SDK targets stay inside the owner.
#[derive(Clone, Serialize)]
pub struct ResolvedModel {
    pub role: ModelRole,
    pub resource: ResourceMetadata,
    pub parameters: Value,
    pub max_calls_per_day: u32,
    pub fingerprint: String,
    #[serde(skip)]
    target: ServiceTarget,
}
impl std::fmt::Debug for ResolvedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedModel")
            .field("role", &self.role)
            .field("resource", &self.resource)
            .field("parameters", &self.parameters)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}
impl ResolvedModel {
    pub(super) fn into_target(self) -> ServiceTarget {
        self.target
    }
}

#[derive(Clone)]
pub struct ModelResources {
    paths: InstancePaths,
    config: ModelConfig,
}
impl ModelResources {
    pub fn new(paths: &InstancePaths, config: &ModelConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            paths: paths.clone(),
            config: config.clone(),
        })
    }
    fn binding(&self, role: ModelRole) -> Option<(&ModelResource, Value, u32)> {
        match role {
            ModelRole::MemoryFormation=>self.config.memory_formation.as_ref().map(|c|(&c.resource,
                json!({"max_input_bytes":c.max_input_bytes,"max_output_tokens":c.max_output_tokens,"native_parameters":c.native_parameters}),c.max_calls_per_day)),
            ModelRole::StructuredFormation=>self.config.structured_formation.as_ref().map(|c|(&c.resource,
                json!({"max_input_bytes":c.max_input_bytes,"max_output_tokens":c.max_output_tokens,"native_parameters":c.native_parameters}),c.max_calls_per_day)),
            ModelRole::TextEmbedding=>self.config.text_embedding.as_ref().map(|c|(&c.resource,
                json!({"max_input_bytes":c.max_input_bytes,"max_batch_size":c.max_batch_size,"dimension":c.dimension,"expected_provider_model":c.expected_provider_model}),c.max_calls_per_day)),
        }
    }
    /// Local configuration and protected credential readiness; zero provider calls.
    pub fn readiness(&self, role: ModelRole) -> ModelReadiness {
        let Some((resource, _, _)) = self.binding(role) else {
            return ModelReadiness {
                role,
                state: ModelReadinessState::WaitingConfiguration,
                resource: None,
                reason: Some("model role is not configured".into()),
            };
        };
        let ready = self.paths.read_secret(&resource.credential).is_ok();
        ModelReadiness {
            role,
            state: if ready {
                ModelReadinessState::Ready
            } else {
                ModelReadinessState::WaitingConfiguration
            },
            resource: Some(resource.into()),
            reason: if ready {
                None
            } else {
                Some("model credential is missing or unreadable".into())
            },
        }
    }
    pub fn resolve(
        &self,
        role: ModelRole,
        requirements: &ModelRequirements,
    ) -> Result<ResolvedModel, ModelFailure> {
        let (resource, parameters, max_calls_per_day) = self
            .binding(role)
            .ok_or_else(|| ModelFailure::waiting("model role is not configured"))?;
        requirements.validate(resource.protocol)?;
        let secret = self
            .paths
            .read_secret(&resource.credential)
            .map_err(|error| {
                ModelFailure::waiting("model credential is missing or unreadable")
                    .with_source(error.into_boxed_dyn_error())
            })?;
        let resource_metadata = ResourceMetadata::from(resource);
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&resource_metadata, &parameters, max_calls_per_day))
                    .expect("resolved model metadata is serializable")
            )
        );
        Ok(ResolvedModel {
            role,
            resource: resource_metadata,
            parameters,
            max_calls_per_day,
            fingerprint,
            target: ServiceTarget {
                endpoint: Endpoint::from_owned(resource.endpoint.clone()),
                auth: AuthData::from_single(secret),
                model: ModelIden::new(AdapterKind::OpenAI, resource.model.clone()),
            },
        })
    }
    pub(super) fn inspect(&self, resource: &ModelResource) -> Value {
        let ready = self.paths.read_secret(&resource.credential).is_ok();
        json!({"state":if ready {"ready"}else{"waiting_configuration"},
            "reason":if ready {None}else{Some("model credential is missing or unreadable")},"resource":ResourceMetadata::from(resource)})
    }
}
