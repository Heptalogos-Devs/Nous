use super::{ModelFailure, ModelFailureInfo, RetryClass};
use chrono::Utc;
use genai::{
    Client, ServiceTarget,
    chat::{ChatOptions, ChatRequest},
    embed::{EmbedOptions, EmbedRequest, EmbedResponse},
};
use nous_domain::ErrorCode;
use serde::Serialize;
use serde_json::Value;
use std::fmt;

/// Provider-specific request types are private to the shared model adapter.
#[derive(Clone)]
pub struct PreparedCall<M> {
    pub metadata: M,
    pub attempt_policy: AttemptPolicy,
    pub(super) target: ServiceTarget,
    pub(super) request: PreparedRequest,
}
#[derive(Clone)]
pub(super) enum PreparedRequest {
    Chat {
        request: ChatRequest,
        options: Box<ChatOptions>,
    },
    Embedding {
        request: EmbedRequest,
        options: Box<EmbedOptions>,
        space: nous_domain::EmbeddingSpace,
        count: usize,
    },
}
pub(super) enum ProviderOutput {
    Text(String),
    Vectors(Vec<Vec<f32>>),
}

#[derive(Debug, Clone, Serialize)]
pub struct CallReservation {
    pub call_id: nous_domain::ModelCallId,
    pub role: String,
    pub resource: super::ResourceMetadata,
    pub request: Value,
    pub attempt_policy: AttemptPolicy,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct AttemptPolicy {
    pub max_attempts: u32,
    pub has_fallback: bool,
    pub retry_uncertain: bool,
}
#[derive(Debug, Clone)]
pub struct AttemptContext {
    pub index: u32,
    pub previous_request: Option<Value>,
}
pub trait CallMetadata: Serialize {
    fn reservation_fields(&self) -> (&str, &super::ResourceMetadata);
    fn role(&self) -> &str;
}
impl<M: CallMetadata> PreparedCall<M> {
    pub fn reservation(&self) -> CallReservation {
        let (call_id, resource) = self.metadata.reservation_fields();
        CallReservation {
            call_id: nous_domain::ModelCallId(call_id.into()),
            role: self.metadata.role().into(),
            resource: resource.clone(),
            request: serde_json::to_value(&self.metadata)
                .expect("prepared metadata is serializable"),
            attempt_policy: self.attempt_policy,
        }
    }
}
impl<M: fmt::Debug> fmt::Debug for PreparedCall<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedCall")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ModelCallRecord<M> {
    pub metadata: M,
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
pub struct ModelExecution<M, T> {
    pub record: ModelCallRecord<M>,
    pub result: Result<T, ModelFailure>,
}
impl<M, T> ModelExecution<M, T> {
    pub fn map<U>(
        mut self,
        parse: impl FnOnce(T, &M) -> Result<U, ModelFailure>,
    ) -> ModelExecution<M, U> {
        let result = self
            .result
            .and_then(|value| parse(value, &self.record.metadata));
        self.record.complete(&result);
        ModelExecution {
            record: self.record,
            result,
        }
    }
}
impl<M> ModelCallRecord<M> {
    fn started(metadata: M) -> Self {
        Self {
            metadata,
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
        }
    }
    fn complete<T>(&mut self, result: &Result<T, ModelFailure>) {
        self.completed_at = Utc::now().to_rfc3339();
        self.status = match result {
            Ok(_) => "succeeded",
            Err(error) if error.class() == RetryClass::Uncertain => "uncertain",
            Err(_) => "failed",
        }
        .into();
        self.failure = result.as_ref().err().map(|error| {
            let mut info = (*error.info).clone();
            info.error = crate::observability::contextualize(info.error);
            info
        });
    }
    fn capture_error(&mut self, error: &genai::Error) {
        if let genai::Error::ChatResponseGeneration { response_body, .. } = error {
            self.provider_response = Some((**response_body).clone());
            self.raw_output = Some(response_body.to_string());
        } else if let Some(body) = error_body(error) {
            self.raw_output = Some(body.into());
            self.provider_response = serde_json::from_str(body).ok();
        }
    }
}

#[derive(Clone)]
pub(super) struct CallExecutor {
    client: Client,
}
impl CallExecutor {
    pub fn new(http: reqwest::Client) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::builder().with_reqwest(http).build()?,
        })
    }
    /// Exactly one external attempt. ModelCalls owns reservations and retry policy.
    pub async fn invoke<M>(&self, prepared: PreparedCall<M>) -> ModelExecution<M, ProviderOutput> {
        let mut record = ModelCallRecord::started(prepared.metadata);
        let result = match prepared.request {
            PreparedRequest::Chat { request, options } => {
                self.chat(prepared.target, request, *options, &mut record)
                    .await
            }
            PreparedRequest::Embedding {
                request,
                options,
                space,
                count,
            } => {
                self.embed(
                    prepared.target,
                    request,
                    *options,
                    &mut record,
                    &space,
                    count,
                )
                .await
            }
        };
        record.complete(&result);
        ModelExecution { record, result }
    }
    async fn chat<M>(
        &self,
        target: ServiceTarget,
        request: ChatRequest,
        options: ChatOptions,
        record: &mut ModelCallRecord<M>,
    ) -> Result<ProviderOutput, ModelFailure> {
        match self.client.exec_chat(target, request, Some(&options)).await {
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
                    Ok(ProviderOutput::Text(
                        record.raw_output.clone().unwrap_or_default(),
                    ))
                }
            }
            Err(error) => {
                record.capture_error(&error);
                Err(ModelFailure::provider(error))
            }
        }
    }
    async fn embed<M>(
        &self,
        target: ServiceTarget,
        request: EmbedRequest,
        options: EmbedOptions,
        record: &mut ModelCallRecord<M>,
        space: &nous_domain::EmbeddingSpace,
        count: usize,
    ) -> Result<ProviderOutput, ModelFailure> {
        match self
            .client
            .exec_embed(target, request, Some(&options))
            .await
        {
            Ok(response) => {
                record.provider_model = Some(response.provider_model_iden.model_name.to_string());
                record.usage = serde_json::to_value(&response.usage).ok();
                record.provider_response = response.captured_raw_body.clone();
                record.raw_output = response.captured_raw_body.as_ref().map(Value::to_string);
                validate_embeddings(response, space, count).map(ProviderOutput::Vectors)
            }
            Err(error) => {
                record.capture_error(&error);
                let invalid = matches!(&error, genai::Error::StreamParse { .. });
                let mut failure = ModelFailure::provider(error);
                if invalid {
                    failure.info.error.code = ErrorCode::InvalidArgument;
                    failure.info.error.message =
                        "embedding response does not match the embedding JSON contract".into();
                }
                Err(failure)
            }
        }
    }
}
fn validate_embeddings(
    response: EmbedResponse,
    space: &nous_domain::EmbeddingSpace,
    count: usize,
) -> Result<Vec<Vec<f32>>, ModelFailure> {
    let invalid =
        |message| ModelFailure::new(RetryClass::Permanent, ErrorCode::InvalidArgument, message);
    if response.provider_model_iden.model_name != space.provider_model {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::Conflict,
            "embedding provider model differs from the configured immutable space",
        ));
    }
    if response.embeddings.len() != count {
        return Err(invalid(
            "embedding response must cover every input exactly once",
        ));
    }
    let mut ordered = vec![None; count];
    for embedding in response.embeddings {
        if embedding.index >= count || ordered[embedding.index].is_some() {
            return Err(invalid(
                "embedding response contains a duplicate or out-of-range index",
            ));
        }
        let vector = embedding.vector;
        if vector.len() != space.dimension
            || !vector.iter().all(|v| v.is_finite())
            || !vector.iter().any(|v| *v != 0.0)
        {
            return Err(invalid(
                "embedding response must contain finite nonzero vectors with the configured dimension",
            ));
        }
        ordered[embedding.index] = Some(vector);
    }
    ordered
        .into_iter()
        .map(|v| v.ok_or_else(|| invalid("embedding response is missing an input index")))
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
