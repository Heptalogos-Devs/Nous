//! Model target resolution and structured formation are adapted from rust-genai's
//! `examples/c06-target-resolver.rs`, `examples/c04-chat-options.rs` and JsonSpec.
//! Upstream: jeremychone/rust-genai, Apache-2.0 OR MIT; release 0.7.0,
//! commit 8fc13af4db80acd0771c87f6180d79b83c9a898d.

use std::{collections::BTreeMap, error::Error as StdError, fmt};

use anyhow::{Result as ConfigResult, bail};
use chrono::Utc;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, JsonSpec};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget, adapter::AdapterKind};
use nous_domain::{
    Error, ErrorCode, MaterialId, MemoryDraft, NamespaceId, ObservationId, SubjectId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::instance::{InstancePaths, SecretRef};

pub const FORMATION_PROMPT_VERSION: &str = "nous.memory.formation.v1";
const FORMATION_PROMPT: &str = include_str!("prompts/memory-formation-v1.md");
const OWNED_REQUEST_FIELDS: &[&str] = &[
    "model",
    "messages",
    "stream",
    "tools",
    "tool_choice",
    "response_format",
    "max_tokens",
    "max_completion_tokens",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub memory_formation: Option<FormationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormationConfig {
    pub resource: ModelResource,
    #[serde(default = "prompt_version")]
    pub prompt_version: String,
    #[serde(default = "input_limit")]
    pub max_input_bytes: usize,
    #[serde(default = "output_limit")]
    pub max_output_tokens: u32,
    #[serde(default = "daily_limit")]
    pub max_calls_per_day: u32,
    #[serde(default)]
    pub native_parameters: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelResource {
    #[serde(default = "resource_id")]
    pub id: String,
    pub provider: String,
    pub protocol: ModelProtocol,
    pub endpoint: String,
    pub model: String,
    pub credential: SecretRef,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub enum ModelProtocol {
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
}

fn prompt_version() -> String {
    FORMATION_PROMPT_VERSION.into()
}
fn resource_id() -> String {
    "memory-formation".into()
}
fn input_limit() -> usize {
    32 * 1024
}
fn output_limit() -> u32 {
    1200
}
fn daily_limit() -> u32 {
    20
}

impl ModelConfig {
    /// Configuration validation performs no network or credential access.
    pub fn validate(&self) -> ConfigResult<()> {
        let Some(config) = &self.memory_formation else {
            return Ok(());
        };
        for (name, value) in [
            ("resource id", &config.resource.id),
            ("provider", &config.resource.provider),
            ("model", &config.resource.model),
        ] {
            if value.trim().is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
                bail!("model {name} must contain 1 to 255 visible bytes");
            }
        }
        let endpoint = reqwest::Url::parse(&config.resource.endpoint)?;
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
        if config.prompt_version != FORMATION_PROMPT_VERSION {
            bail!("unsupported memory formation prompt version");
        }
        if !(1..=1024 * 1024).contains(&config.max_input_bytes) {
            bail!("models.memory_formation.max_input_bytes must be between 1 and 1048576");
        }
        if !(1..=32768).contains(&config.max_output_tokens) || config.max_calls_per_day > 100000 {
            bail!("model output tokens must be 1 to 32768 and daily calls at most 100000");
        }
        for field in config.native_parameters.keys() {
            if OWNED_REQUEST_FIELDS.contains(&field.as_str()) {
                bail!("native parameter '{field}' is owned by the formation request contract");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationInput {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub material_id: MaterialId,
    pub material_commit_seq: i64,
    pub observation_id: ObservationId,
    pub observation_commit_seq: i64,
    #[serde(default)]
    pub source_event_kind: nous_domain::ObservationKind,
    pub text: String,
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

#[derive(Debug, Clone, Serialize)]
pub struct PromptMetadata {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCallMetadata {
    pub schema_version: u32,
    pub call_id: String,
    pub role: String,
    pub resource: ResourceMetadata,
    pub prompt: PromptMetadata,
    pub input: FormationInput,
    pub rendered_user: String,
    pub parameters: Value,
    pub max_calls_per_day: u32,
}

#[derive(Clone)]
pub struct PreparedFormation {
    pub metadata: ModelCallMetadata,
    client: Client,
    request: ChatRequest,
    options: ChatOptions,
}

impl fmt::Debug for PreparedFormation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedFormation")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    WaitingConfiguration,
    Retryable,
    Permanent,
    Uncertain,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelFailureInfo {
    pub class: RetryClass,
    pub code: ErrorCode,
    pub http_status: Option<u16>,
    pub retry_after_seconds: Option<u64>,
    pub message: String,
}

/// Retains the SDK error for typed inspection while the persisted view is redacted.
pub struct ModelFailure {
    pub info: ModelFailureInfo,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl ModelFailure {
    pub fn class(&self) -> RetryClass {
        self.info.class
    }
    pub fn to_domain_error(&self) -> Error {
        Error::new(self.info.code, self.info.message.clone())
    }
    fn new(class: RetryClass, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            info: ModelFailureInfo {
                class,
                code,
                http_status: None,
                retry_after_seconds: None,
                message: message.into(),
            },
            source: None,
        }
    }
    fn waiting(message: impl Into<String>) -> Self {
        Self::new(
            RetryClass::WaitingConfiguration,
            ErrorCode::CapabilityUnavailable,
            message,
        )
    }
    fn provider(error: genai::Error) -> Self {
        let status = error.status().map(|value| value.as_u16());
        let class = match status {
            Some(408) => RetryClass::Uncertain,
            Some(429 | 500..=599) => RetryClass::Retryable,
            Some(_) => RetryClass::Permanent,
            None => match &error {
                genai::Error::WebModelCall {
                    webc_error: genai::webc::Error::Reqwest(error),
                    ..
                }
                | genai::Error::WebAdapterCall {
                    webc_error: genai::webc::Error::Reqwest(error),
                    ..
                } if error.is_builder() => RetryClass::Permanent,
                genai::Error::WebModelCall {
                    webc_error: genai::webc::Error::Reqwest(error),
                    ..
                }
                | genai::Error::WebAdapterCall {
                    webc_error: genai::webc::Error::Reqwest(error),
                    ..
                } if error.is_connect() => RetryClass::Retryable,
                genai::Error::WebModelCall { .. }
                | genai::Error::WebAdapterCall { .. }
                | genai::Error::ChatResponseGeneration { .. }
                | genai::Error::NoChatResponse { .. } => RetryClass::Uncertain,
                _ => RetryClass::Permanent,
            },
        };
        let retry_after_seconds = error
            .headers()
            .and_then(|headers| headers.get("retry-after"))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        let message = status.map_or_else(
            || "model request failed; typed provider cause retained".to_owned(),
            |status| format!("model provider returned HTTP {status}"),
        );
        Self {
            info: ModelFailureInfo {
                class,
                code: ErrorCode::DependencyUnavailable,
                http_status: status,
                retry_after_seconds,
                message,
            },
            source: Some(Box::new(error)),
        }
    }
}

impl fmt::Debug for ModelFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.info.fmt(formatter)
    }
}
impl fmt::Display for ModelFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.info.message.fmt(formatter)
    }
}
impl StdError for ModelFailure {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn StdError + 'static))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCallRecord {
    pub schema_version: u32,
    pub metadata: ModelCallMetadata,
    pub started_at: String,
    pub completed_at: String,
    pub status: String,
    pub provider_model: Option<String>,
    pub usage: Option<Value>,
    pub raw_output: Option<String>,
    pub provider_response: Option<Value>,
    pub stop_reason: Option<String>,
    pub cost: Option<Value>,
    pub failure: Option<ModelFailureInfo>,
}

pub struct FormationExecution {
    pub record: ModelCallRecord,
    pub result: Result<Vec<MemoryDraft>, ModelFailure>,
}

#[derive(Clone)]
pub struct ModelRuntime {
    paths: InstancePaths,
    config: ModelConfig,
    http: reqwest::Client,
}

impl ModelRuntime {
    pub fn new(
        paths: &InstancePaths,
        config: &ModelConfig,
        http: reqwest::Client,
    ) -> ConfigResult<Self> {
        config.validate()?;
        Ok(Self {
            paths: paths.clone(),
            config: config.clone(),
            http,
        })
    }

    /// Reads only local readiness; never calls a provider or exposes credential bytes.
    pub fn inspect(&self) -> Value {
        let Some(config) = &self.config.memory_formation else {
            return json!({"state":"waiting_configuration","reason":"memory formation resource is not configured"});
        };
        let ready = self.paths.read_secret(&config.resource.credential).is_ok();
        json!({"state":if ready {"ready"} else {"waiting_configuration"},
            "reason":if ready {None} else {Some("model credential is missing or unreadable")},
            "resource":ResourceMetadata::from(&config.resource),"prompt_version":config.prompt_version,
            "max_input_bytes":config.max_input_bytes,"max_output_tokens":config.max_output_tokens,
            "max_calls_per_day":config.max_calls_per_day,"native_parameters":config.native_parameters,"cost":null})
    }

    /// Prepare first, then persistently reserve metadata.call_id before execute.
    pub fn prepare(&self, input: FormationInput) -> Result<PreparedFormation, ModelFailure> {
        let config =
            self.config.memory_formation.as_ref().ok_or_else(|| {
                ModelFailure::waiting("memory formation resource is not configured")
            })?;
        if input.text.trim().is_empty()
            || input.material_commit_seq <= 0
            || input.observation_commit_seq <= 0
        {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                ErrorCode::InvalidArgument,
                "formation requires nonempty source text and positive source versions",
            ));
        }
        if input.text.len() > config.max_input_bytes {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                ErrorCode::ResourceExhausted,
                "formation source exceeds max_input_bytes",
            ));
        }
        let secret = self
            .paths
            .read_secret(&config.resource.credential)
            .map_err(|error| {
                let mut failure =
                    ModelFailure::waiting("model credential is missing or unreadable");
                failure.source = Some(error.into_boxed_dyn_error());
                failure
            })?;
        let resource = &config.resource;
        let endpoint = Endpoint::from_owned(resource.endpoint.clone());
        let model = resource.model.clone();
        // Copied from the upstream target-resolver example; every route component
        // is explicit, so gateway model names never select an inferred provider.
        let resolver = ServiceTargetResolver::from_resolver_fn(move |_target: ServiceTarget| {
            Ok(ServiceTarget {
                endpoint: endpoint.clone(),
                auth: AuthData::from_single(secret.clone()),
                model: ModelIden::new(AdapterKind::OpenAI, model.clone()),
            })
        });
        let client = Client::builder()
            .with_reqwest(self.http.clone())
            .with_adapter_kind(AdapterKind::OpenAI)
            .with_service_target_resolver(resolver)
            .build()
            .map_err(ModelFailure::provider)?;
        let rendered_user = serde_json::to_string(
            &json!({"material_text": &input.text,"source_event_kind":input.source_event_kind}),
        )
        .expect("formation text has a JSON representation");
        if FORMATION_PROMPT.len() + rendered_user.len() > config.max_input_bytes {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                ErrorCode::ResourceExhausted,
                "rendered formation input exceeds max_input_bytes",
            ));
        }
        let request = ChatRequest::new(vec![ChatMessage::user(rendered_user.clone())])
            .with_system(FORMATION_PROMPT);
        let options = ChatOptions::default()
            .with_max_tokens(config.max_output_tokens)
            .with_capture_raw_body(true)
            .with_response_format(JsonSpec::new("nous_memory_formation_v1", output_schema()));
        let options = if config.native_parameters.is_empty() {
            options
        } else {
            options.with_extra_body(json!(&config.native_parameters))
        };
        let metadata = ModelCallMetadata {
            schema_version: 1,
            call_id: uuid::Uuid::new_v4().to_string(),
            role: "memory_formation".into(),
            resource: resource.into(),
            prompt: PromptMetadata {
                id: "memory_formation".into(),
                version: config.prompt_version.clone(),
                sha256: format!("{:x}", Sha256::digest(FORMATION_PROMPT.as_bytes())),
                text: FORMATION_PROMPT.into(),
            },
            input,
            rendered_user,
            parameters: json!({"max_output_tokens":config.max_output_tokens,
                "max_input_bytes":config.max_input_bytes,"response_format":"json_schema",
                "native_parameters":&config.native_parameters,
                "genai_chat_options":&options}),
            max_calls_per_day: config.max_calls_per_day,
        };
        Ok(PreparedFormation {
            metadata,
            client,
            request,
            options,
        })
    }

    /// Exactly one SDK invocation. Durable retry and uncertain-result handling belong to the worker.
    pub async fn execute(&self, prepared: PreparedFormation) -> FormationExecution {
        let mut record = ModelCallRecord {
            schema_version: 1,
            metadata: prepared.metadata,
            started_at: Utc::now().to_rfc3339(),
            completed_at: String::new(),
            status: "started".into(),
            provider_model: None,
            usage: None,
            raw_output: None,
            provider_response: None,
            stop_reason: None,
            cost: None,
            failure: None,
        };
        let model = ModelIden::new(AdapterKind::OpenAI, record.metadata.resource.model.clone());
        let result = match prepared
            .client
            .exec_chat(model, prepared.request, Some(&prepared.options))
            .await
        {
            Ok(response) => {
                record.provider_response = response.captured_raw_body.clone();
                record.provider_model = Some(response.provider_model_iden.model_name.to_string());
                record.usage = serde_json::to_value(&response.usage).ok();
                record.stop_reason = response.stop_reason.as_ref().map(ToString::to_string);
                record.raw_output = response.first_text().map(str::to_owned);
                if response
                    .stop_reason
                    .as_ref()
                    .is_some_and(|reason| reason.is_max_tokens())
                {
                    Err(ModelFailure::new(
                        RetryClass::Permanent,
                        ErrorCode::ResourceExhausted,
                        "model output was truncated by max_output_tokens",
                    ))
                } else {
                    parse_drafts(record.raw_output.as_deref().unwrap_or_default())
                }
            }
            Err(error) => {
                if let genai::Error::ChatResponseGeneration { response_body, .. } = &error {
                    record.provider_response = Some((**response_body).clone());
                }
                Err(ModelFailure::provider(error))
            }
        };
        record.completed_at = Utc::now().to_rfc3339();
        record.status = match &result {
            Ok(_) => "succeeded",
            Err(error) if error.class() == RetryClass::Uncertain => "uncertain",
            Err(_) => "failed",
        }
        .into();
        record.failure = result.as_ref().err().map(|error| error.info.clone());
        FormationExecution { record, result }
    }
}

fn output_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["memories"],"properties":{
        "memories":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"object","additionalProperties":false,
            "required":["title","content","epistemic_status"],"properties":{
                "title":{"type":"string","minLength":1,"maxLength":256},
                "content":{"type":"string","minLength":1,"maxLength":16384},
                "epistemic_status":{"type":"string","enum":["reported","inferred","uncertain"]}}}}}})
}

fn parse_drafts(text: &str) -> Result<Vec<MemoryDraft>, ModelFailure> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Output {
        memories: Vec<Draft>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Draft {
        title: String,
        content: String,
        epistemic_status: String,
    }
    let output: Output = serde_json::from_str(text).map_err(|error| {
        let mut failure = ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::InvalidArgument,
            "model output does not match the formation JSON contract",
        );
        failure.source = Some(Box::new(error));
        failure
    })?;
    if !(1..=3).contains(&output.memories.len())
        || output.memories.iter().any(|draft| {
            draft.title.trim().is_empty()
                || draft.title.chars().count() > 256
                || draft.content.trim().is_empty()
                || draft.content.chars().count() > 16384
                || !matches!(
                    draft.epistemic_status.as_str(),
                    "reported" | "inferred" | "uncertain"
                )
        })
    {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::InvalidArgument,
            "model output must contain 1 to 3 bounded memories with valid epistemic status",
        ));
    }
    Ok(output
        .memories
        .into_iter()
        .map(|draft| MemoryDraft {
            title: draft.title,
            content: draft.content,
            epistemic_status: draft.epistemic_status,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_parameters_keep_provider_controls_and_protect_owned_fields() {
        let config: ModelConfig = serde_json::from_value(json!({"memory_formation":{
            "resource":{"provider":"new-api","protocol":"openai_chat_completions",
                "endpoint":"http://127.0.0.1:3000/v1/","model":"doubao-seed-2.1-lite",
                "credential":{"file":"credentials/model-gateway.token"}},
            "native_parameters":{"thinking":{"type":"disabled"}}
        }}))
        .unwrap();
        config.validate().unwrap();
        for field in OWNED_REQUEST_FIELDS {
            let mut invalid = config.clone();
            invalid
                .memory_formation
                .as_mut()
                .unwrap()
                .native_parameters
                .insert((*field).into(), json!("override"));
            assert!(invalid.validate().is_err(), "reserved field {field}");
        }
        let parameters = &config.memory_formation.unwrap().native_parameters;
        let options = ChatOptions::default().with_extra_body(json!(parameters));
        assert_eq!(options.extra_body.unwrap()["thinking"]["type"], "disabled");
    }

    #[test]
    fn formation_requires_bounded_strict_structured_output() {
        let valid = r#"{"memories":[{"title":"见闻","content":"资料记载红叶在秋天变色。","epistemic_status":"reported"}]}"#;
        let memories = parse_drafts(valid).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].epistemic_status, "reported");
        for invalid in [
            r#"{"memories":[]}"#,
            r#"{"memories":[{"title":"x","content":"x","epistemic_status":"verified"}]}"#,
            r#"{"memories":[{"title":"x","content":"x","epistemic_status":"reported","source":"invented"}]}"#,
            r#"{"memories":[{"title":" ","content":"x","epistemic_status":"reported"}]}"#,
            "```json\n{}\n```",
        ] {
            assert_eq!(
                parse_drafts(invalid).unwrap_err().class(),
                RetryClass::Permanent
            );
        }
    }

    #[test]
    fn provider_status_and_typed_cause_survive_redaction() {
        for (status, expected) in [
            (401, RetryClass::Permanent),
            (429, RetryClass::Retryable),
            (503, RetryClass::Retryable),
            (408, RetryClass::Uncertain),
        ] {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("retry-after", "7".parse().unwrap());
            let failure = ModelFailure::provider(genai::Error::HttpError {
                status: reqwest::StatusCode::from_u16(status).unwrap(),
                canonical_reason: "provider error".into(),
                body: "private provider body".into(),
                headers: Box::new(headers),
            });
            assert_eq!(failure.class(), expected);
            assert_eq!(failure.info.http_status, Some(status));
            assert_eq!(failure.info.retry_after_seconds, Some(7));
            assert!(StdError::source(&failure).unwrap().is::<genai::Error>());
            assert!(!format!("{failure:?}").contains("private provider body"));
        }
    }
}
