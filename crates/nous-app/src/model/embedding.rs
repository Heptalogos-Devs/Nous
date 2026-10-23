//! Text embeddings adapted from rust-genai 0.7.0's OpenAI embedding adapter and
//! `EmbedRequest::new_batch` API. Upstream jeremychone/rust-genai, MIT OR Apache-2.0,
//! commit 8fc13af4db80acd0771c87f6180d79b83c9a898d. Uses the same explicit target
//! and shared HTTP client as formation; durable dispatch belongs to the worker.

use std::{collections::BTreeSet, fmt};

use anyhow::{Result as ConfigResult, bail};
use chrono::Utc;
use genai::{
    Client, ServiceTarget,
    embed::{EmbedOptions, EmbedRequest, EmbedResponse},
};
use nous_domain::{EmbeddingInputItem, EmbeddingKind, EmbeddingSpace, ErrorCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    ModelFailure, ModelFailureInfo, ModelProtocol, ModelResource, ModelRuntime, ResourceMetadata,
    RetryClass, validate_resource,
};

pub const EMBEDDING_PRODUCER_VERSION: &str = "nous.embedding.text.v1";

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
    #[serde(default = "producer_version")]
    pub producer_version: String,
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
fn producer_version() -> String {
    EMBEDDING_PRODUCER_VERSION.into()
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
        if !(1..=65_536).contains(&self.dimension) {
            bail!("embedding dimension must be between 1 and 65536");
        }
        if !(1..=256).contains(&self.max_batch_size) {
            bail!("embedding max_batch_size must be between 1 and 256");
        }
        if !(1..=1024 * 1024).contains(&self.max_input_bytes) || self.max_calls_per_day > 100_000 {
            bail!("embedding input bytes must be 1 to 1048576 and daily calls at most 100000");
        }
        if self.producer_version != EMBEDDING_PRODUCER_VERSION {
            bail!("unsupported text embedding producer version");
        }
        Ok(())
    }

    /// Secret-free immutable identity. Execution budgets and batching do not
    /// change vector meaning, while every producer/space input is bound here.
    pub fn space(&self) -> EmbeddingSpace {
        let material = serde_json::to_vec(&(
            1u32,
            "text",
            ResourceMetadata::from(&self.resource),
            &self.expected_provider_model,
            self.dimension,
            &self.producer_version,
        ))
        .expect("embedding identity contains JSON-serializable fields");
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
    pub schema_version: u32,
    pub call_id: String,
    pub role: String,
    pub resource: ResourceMetadata,
    pub space: EmbeddingSpace,
    pub producer_signature: String,
    pub inputs: Vec<EmbeddingInputItem>,
    pub parameters: Value,
    pub max_calls_per_day: u32,
}

#[derive(Clone)]
pub struct PreparedEmbedding {
    pub metadata: EmbeddingCallMetadata,
    client: Client,
    target: ServiceTarget,
    request: EmbedRequest,
    options: EmbedOptions,
}

impl fmt::Debug for PreparedEmbedding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedEmbedding")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingCallRecord {
    pub schema_version: u32,
    pub metadata: EmbeddingCallMetadata,
    pub started_at: String,
    pub completed_at: String,
    pub status: String,
    pub provider_model: Option<String>,
    pub usage: Option<Value>,
    pub raw_output: Option<String>,
    pub provider_response: Option<Value>,
    pub cost: Option<Value>,
    pub failure: Option<ModelFailureInfo>,
}

pub struct EmbeddingExecution {
    pub record: EmbeddingCallRecord,
    pub result: Result<Vec<Vec<f32>>, ModelFailure>,
}

impl ModelRuntime {
    pub fn embedding_config(&self) -> Option<&EmbeddingConfig> {
        self.config.text_embedding.as_ref()
    }

    /// Local readiness only; credentials are never exposed and no provider is called.
    pub fn embedding_inspect(&self) -> Value {
        let Some(config) = self.embedding_config() else {
            return json!({"state":"waiting_configuration","reason":"text embedding resource is not configured"});
        };
        let ready = self.paths.read_secret(&config.resource.credential).is_ok();
        json!({"state":if ready {"ready"} else {"waiting_configuration"},
            "reason":if ready {None} else {Some("model credential is missing or unreadable")},
            "resource":ResourceMetadata::from(&config.resource), "space":config.space(),
            "producer_version":config.producer_version,"expected_provider_model":config.expected_provider_model,
            "dimension":config.dimension,"max_batch_size":config.max_batch_size,
            "max_input_bytes":config.max_input_bytes,"max_calls_per_day":config.max_calls_per_day,"cost":null})
    }

    /// Prepare without an external call. Reserve metadata.call_id in the existing
    /// model ledger before handing this value to execute_embedding.
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
            if item.text.trim().is_empty() {
                return Err(invalid("embedding requires nonempty source text"));
            }
            bytes = bytes
                .checked_add(item.text.len())
                .ok_or_else(|| exhausted("embedding input size overflow"))?;
            if bytes > config.max_input_bytes {
                return Err(exhausted("embedding source exceeds max_input_bytes"));
            }
            let hash = format!("{:x}", Sha256::digest(item.text.as_bytes()));
            let key = match item.kind {
                EmbeddingKind::Memory => match (&item.memory_id, item.memory_revision) {
                    (Some(id), Some(revision)) if revision > 0 => format!("memory:{id}@{revision}"),
                    _ => {
                        return Err(invalid(
                            "memory embedding requires a memory ID and positive revision",
                        ));
                    }
                },
                EmbeddingKind::Tag | EmbeddingKind::Query => {
                    if item.memory_id.is_some()
                        || item.memory_revision.is_some()
                        || item.text != item.text.trim()
                    {
                        return Err(invalid(
                            "tag/query embedding requires trimmed text without a memory reference",
                        ));
                    }
                    format!(
                        "{}:{hash}",
                        if item.kind == EmbeddingKind::Tag {
                            "tag"
                        } else {
                            "query"
                        }
                    )
                }
            };
            if hash != item.source_hash
                || key != item.source_key
                || !keys.insert(item.source_key.clone())
            {
                return Err(invalid(
                    "embedding source identity/hash is invalid or duplicated",
                ));
            }
        }
        let request =
            EmbedRequest::new_batch(inputs.iter().map(|item| item.text.clone()).collect());
        let space = config.space();
        let (client, target) = self.build_client(&config.resource)?;
        Ok(PreparedEmbedding {
            metadata: EmbeddingCallMetadata {
                schema_version: 1,
                call_id: uuid::Uuid::new_v4().to_string(),
                role: "text_embedding".into(),
                resource: (&config.resource).into(),
                producer_signature: space.producer_signature.clone(),
                space,
                inputs,
                parameters: json!({"encoding_format":"float","max_batch_size":config.max_batch_size,
                    "max_input_bytes":config.max_input_bytes,"producer_version":config.producer_version}),
                max_calls_per_day: config.max_calls_per_day,
            },
            client,
            target,
            request,
            options: EmbedOptions::new()
                .with_encoding_format("float")
                .with_capture_raw_body(true),
        })
    }

    /// Exactly one SDK invocation. Durable retry, crash recovery and adoption are
    /// centralized in the worker and existing model-call ledger.
    pub async fn execute_embedding(&self, prepared: PreparedEmbedding) -> EmbeddingExecution {
        let mut record = EmbeddingCallRecord {
            schema_version: 1,
            metadata: prepared.metadata,
            started_at: Utc::now().to_rfc3339(),
            completed_at: String::new(),
            status: "started".into(),
            provider_model: None,
            usage: None,
            raw_output: None,
            provider_response: None,
            cost: None,
            failure: None,
        };
        let result = match prepared
            .client
            .exec_embed(prepared.target, prepared.request, Some(&prepared.options))
            .await
        {
            Ok(response) => {
                record.provider_model = Some(response.provider_model_iden.model_name.to_string());
                record.usage = serde_json::to_value(&response.usage).ok();
                record.provider_response = response.captured_raw_body.clone();
                record.raw_output = response.captured_raw_body.as_ref().map(Value::to_string);
                validate_response(response, &record.metadata)
            }
            Err(error) => {
                record.raw_output = error_body(&error).map(str::to_owned);
                record.provider_response = record
                    .raw_output
                    .as_deref()
                    .and_then(|body| serde_json::from_str(body).ok());
                let invalid_contract = matches!(&error, genai::Error::StreamParse { .. });
                let mut failure = ModelFailure::provider(error);
                if invalid_contract {
                    failure.info.code = ErrorCode::InvalidArgument;
                    failure.info.message =
                        "embedding response does not match the embedding JSON contract".into();
                }
                Err(failure)
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
        EmbeddingExecution { record, result }
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

fn error_body(error: &genai::Error) -> Option<&str> {
    match error {
        genai::Error::HttpError { body, .. } => Some(body),
        genai::Error::WebModelCall { webc_error, .. }
        | genai::Error::WebAdapterCall { webc_error, .. } => match webc_error {
            genai::webc::Error::ResponseFailedStatus { body, .. }
            | genai::webc::Error::ResponseFailedNotJson { body, .. }
            | genai::webc::Error::ResponseFailedInvalidJson { body, .. } => Some(body),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        instance::{InstancePaths, SecretRef},
        model::ModelConfig,
    };
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    const TOKEN: &str = "local-test-credential-not-a-provider-secret-embedding-0001";

    fn config(endpoint: String) -> EmbeddingConfig {
        EmbeddingConfig {
            resource: ModelResource {
                id: "text-vectors".into(),
                provider: "test".into(),
                protocol: ModelProtocol::OpenAiEmbeddings,
                endpoint,
                model: "private-gateway-alias".into(),
                credential: SecretRef {
                    file: "credentials/embedding-test.token".into(),
                },
            },
            expected_provider_model: "actual-model-2026".into(),
            dimension: 2,
            max_batch_size: 32,
            max_input_bytes: 32 * 1024,
            max_calls_per_day: 100,
            producer_version: producer_version(),
        }
    }

    fn runtime(endpoint: String, timeout: Duration) -> (tempfile::TempDir, ModelRuntime) {
        let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp");
        std::fs::create_dir_all(&temporary_root).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("embedding-model-test-")
            .tempdir_in(temporary_root)
            .unwrap();
        let paths = InstancePaths::new(temporary.path()).unwrap();
        paths.ensure_layout().unwrap();
        std::fs::write(paths.credentials.join("embedding-test.token"), TOKEN).unwrap();
        let model = ModelRuntime::new(
            &paths,
            &ModelConfig {
                memory_formation: None,
                text_embedding: Some(config(endpoint)),
                structured_formation: None,
            },
            reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(timeout.min(Duration::from_millis(100)))
                .timeout(timeout)
                .build()
                .unwrap(),
        )
        .unwrap();
        (temporary, model)
    }

    fn inputs() -> Vec<EmbeddingInputItem> {
        ["潮汐站。", "蓝色旗帜。"]
            .into_iter()
            .map(|text| {
                let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
                EmbeddingInputItem {
                    kind: EmbeddingKind::Query,
                    source_key: format!("query:{hash}"),
                    text: text.into(),
                    memory_id: None,
                    memory_revision: None,
                    source_hash: hash,
                }
            })
            .collect()
    }

    fn response(data: Value) -> Value {
        json!({"object":"list","model":"actual-model-2026","data":data,
            "usage":{"prompt_tokens":3,"total_tokens":3}})
    }

    #[test]
    fn role_config_and_space_identity_exclude_only_execution_controls_and_secrets() {
        let base = config("http://127.0.0.1:1/v1/".into());
        base.validate().unwrap();
        let mut updated = base.clone();
        updated.resource.credential.file = "credentials/rotated.token".into();
        updated.max_calls_per_day = 2;
        updated.max_batch_size = 1;
        updated.max_input_bytes = 64;
        assert_eq!(base.space(), updated.space());
        for field in [
            "id",
            "provider",
            "endpoint",
            "protocol",
            "model",
            "provider_model",
            "dimension",
            "producer",
        ] {
            let mut changed = base.clone();
            match field {
                "id" => changed.resource.id.push('2'),
                "provider" => changed.resource.provider.push('2'),
                "endpoint" => changed.resource.endpoint.push_str("other/"),
                "protocol" => changed.resource.protocol = ModelProtocol::OpenAiChatCompletions,
                "model" => changed.resource.model.push('2'),
                "provider_model" => changed.expected_provider_model.push('2'),
                "dimension" => changed.dimension += 1,
                "producer" => changed.producer_version.push('2'),
                _ => unreachable!(),
            }
            assert_ne!(base.space(), changed.space(), "identity must bind {field}");
        }
        let minimal: EmbeddingConfig = serde_json::from_value(json!({"resource":base.resource,
            "expected_provider_model":"actual-model-2026","dimension":2048}))
        .unwrap();
        assert_eq!(
            (
                minimal.max_batch_size,
                minimal.max_input_bytes,
                minimal.max_calls_per_day
            ),
            (32, 32768, 100)
        );
        let mut wrong = minimal;
        wrong.resource.protocol = ModelProtocol::OpenAiChatCompletions;
        assert!(wrong.validate().is_err());
        let mut wrong_formation: ModelConfig = serde_json::from_value(json!({"memory_formation":{
            "resource":{"provider":"test","protocol":"openai_embeddings","endpoint":"http://localhost/v1/","model":"embed","credential":{"file":"credentials/model.token"}}
        }})).unwrap();
        assert!(wrong_formation.validate().is_err());
        wrong_formation
            .memory_formation
            .as_mut()
            .unwrap()
            .resource
            .protocol = ModelProtocol::OpenAiChatCompletions;
        wrong_formation.validate().unwrap();
    }

    #[tokio::test]
    async fn sdk_reorders_batches_and_retains_provider_evidence_without_credentials() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let provider = Router::new().route("/v1/embeddings", post(move |headers: HeaderMap, Json(request): Json<Value>| {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                assert_eq!(headers["authorization"], format!("Bearer {TOKEN}"));
                assert_eq!(request["model"], "private-gateway-alias");
                assert_eq!(request["encoding_format"], "float");
                assert_eq!(request["input"], json!(["潮汐站。", "蓝色旗帜。"]));
                assert!(request.get("dimensions").is_none());
                Json(response(json!([{"index":1,"embedding":[0.0,1.0]},{"index":0,"embedding":[1.0,0.0]}])))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, provider).await.unwrap();
        });
        let (_temporary, model) = runtime(endpoint, Duration::from_secs(2));
        let prepared = model.prepare_embedding(inputs()).unwrap();
        let metadata = serde_json::to_string(&prepared.metadata).unwrap();
        assert!(!metadata.contains(TOKEN) && !metadata.contains("credential"));
        assert!(!format!("{prepared:?}").contains(TOKEN));
        assert_eq!(model.embedding_inspect()["state"], "ready");
        assert!(!model.embedding_inspect().to_string().contains("credential"));
        let execution = model.execute_embedding(prepared).await;
        assert_eq!(
            execution.result.unwrap(),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );
        assert_eq!(execution.record.status, "succeeded");
        assert_eq!(
            execution.record.provider_model.as_deref(),
            Some("actual-model-2026")
        );
        assert_eq!(execution.record.usage.as_ref().unwrap()["prompt_tokens"], 3);
        assert_eq!(
            execution.record.provider_response.as_ref().unwrap()["data"][0]["index"],
            1
        );
        assert!(execution.record.raw_output.is_some());
        assert!(!execution.record.completed_at.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn sdk_rejects_incomplete_or_incompatible_vectors_and_keeps_raw_results() {
        let cases = [
            (
                "duplicate",
                response(
                    json!([{"index":0,"embedding":[1.0,0.0]},{"index":0,"embedding":[0.0,1.0]}]),
                ),
                ErrorCode::InvalidArgument,
            ),
            (
                "missing",
                response(json!([{"index":0,"embedding":[1.0,0.0]}])),
                ErrorCode::InvalidArgument,
            ),
            (
                "range",
                response(
                    json!([{"index":0,"embedding":[1.0,0.0]},{"index":2,"embedding":[0.0,1.0]}]),
                ),
                ErrorCode::InvalidArgument,
            ),
            (
                "dimension",
                response(
                    json!([{"index":0,"embedding":[1.0,0.0,0.0]},{"index":1,"embedding":[0.0,1.0]}]),
                ),
                ErrorCode::InvalidArgument,
            ),
            (
                "nonfinite",
                response(
                    json!([{"index":0,"embedding":[1e100,0.0]},{"index":1,"embedding":[0.0,1.0]}]),
                ),
                ErrorCode::InvalidArgument,
            ),
            (
                "zero",
                response(
                    json!([{"index":0,"embedding":[0.0,0.0]},{"index":1,"embedding":[0.0,1.0]}]),
                ),
                ErrorCode::InvalidArgument,
            ),
            (
                "model",
                json!({"model":"different-version","data":[{"index":0,"embedding":[1.0,0.0]},{"index":1,"embedding":[0.0,1.0]}],"usage":{"prompt_tokens":3,"total_tokens":3}}),
                ErrorCode::Conflict,
            ),
        ];
        for (name, body, expected_code) in cases {
            let sent = body.clone();
            let provider = Router::new().route(
                "/v1/embeddings",
                post(move || {
                    let sent = sent.clone();
                    async move { Json(sent) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, provider).await.unwrap();
            });
            let (_temporary, model) = runtime(endpoint, Duration::from_secs(2));
            let execution = model
                .execute_embedding(model.prepare_embedding(inputs()).unwrap())
                .await;
            let error = execution.result.unwrap_err();
            assert_eq!(error.class(), RetryClass::Permanent, "{name}");
            assert_eq!(error.info.code, expected_code, "{name}");
            assert_eq!(execution.record.provider_response, Some(body), "{name}");
            assert!(execution.record.failure.is_some());
            assert_eq!(execution.record.usage.as_ref().unwrap()["prompt_tokens"], 3);
            server.abort();
        }
    }

    #[tokio::test]
    async fn sdk_failures_preserve_retry_policy_and_missing_credentials_wait_locally() {
        for (status, expected) in [
            (401, RetryClass::Permanent),
            (429, RetryClass::Retryable),
            (503, RetryClass::Retryable),
            (408, RetryClass::Uncertain),
        ] {
            let provider = Router::new().route(
                "/v1/embeddings",
                post(move || async move {
                    (
                        StatusCode::from_u16(status).unwrap(),
                        [("retry-after", "7")],
                        Json(json!({"error":{"message":"controlled provider failure"}})),
                    )
                        .into_response()
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, provider).await.unwrap();
            });
            let (_temporary, model) = runtime(endpoint, Duration::from_secs(2));
            let execution = model
                .execute_embedding(model.prepare_embedding(inputs()).unwrap())
                .await;
            let error = execution.result.unwrap_err();
            assert_eq!(error.class(), expected);
            assert_eq!(error.info.http_status, Some(status));
            assert_eq!(error.info.retry_after_seconds, Some(7));
            assert!(
                execution
                    .record
                    .raw_output
                    .unwrap()
                    .contains("controlled provider failure")
            );
            assert!(!format!("{error:?}").contains("controlled provider failure"));
            assert_eq!(
                execution.record.status,
                if expected == RetryClass::Uncertain {
                    "uncertain"
                } else {
                    "failed"
                }
            );
            server.abort();
        }
        let (_temporary, model) =
            runtime("http://127.0.0.1:1/v1/".into(), Duration::from_millis(100));
        std::fs::remove_file(model.paths.credentials.join("embedding-test.token")).unwrap();
        assert_eq!(model.embedding_inspect()["state"], "waiting_configuration");
        assert_eq!(
            model.prepare_embedding(inputs()).unwrap_err().class(),
            RetryClass::WaitingConfiguration
        );
        let mut corrupted = inputs();
        corrupted[0].source_hash = "incorrect".into();
        assert_eq!(
            model.prepare_embedding(corrupted).unwrap_err().info.code,
            ErrorCode::InvalidArgument
        );
    }

    #[tokio::test]
    async fn connection_failure_can_retry_but_a_dispatched_timeout_is_uncertain() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        drop(listener);
        let (_temporary, model) = runtime(endpoint, Duration::from_secs(2));
        let execution = model
            .execute_embedding(model.prepare_embedding(inputs()).unwrap())
            .await;
        let failure = execution.result.unwrap_err();
        assert_eq!(
            failure.class(),
            RetryClass::Retryable,
            "{:?}",
            std::error::Error::source(&failure)
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let dispatched = calls.clone();
        let provider = Router::new().route(
            "/v1/embeddings",
            post(move || {
                let dispatched = dispatched.clone();
                async move {
                    dispatched.fetch_add(1, Ordering::SeqCst);
                    std::future::pending::<Json<Value>>().await
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, provider).await.unwrap();
        });
        let (_temporary, model) = runtime(endpoint, Duration::from_millis(200));
        let execution = model
            .execute_embedding(model.prepare_embedding(inputs()).unwrap())
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(execution.result.unwrap_err().class(), RetryClass::Uncertain);
        assert_eq!(execution.record.status, "uncertain");
        server.abort();
    }
}
