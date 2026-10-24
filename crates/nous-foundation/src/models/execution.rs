use super::{ModelFailure, ModelFailureInfo, RetryClass};
use crate::clock::Clock;
use genai::{
    Client, ServiceTarget,
    chat::{ChatOptions, ChatRequest},
    embed::{EmbedOptions, EmbedRequest},
};
use nous_domain::ErrorCode;
use nous_domain::{ModelCost, ModelPriceSnapshot, ModelUsage, UsageKind};
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
        contract: super::openai_embedding::ResponseContract,
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
    fn pricing_snapshot(&self) -> Option<ModelPriceSnapshot>;
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
    pub usage: Option<ModelUsage>,
    pub raw_output: Option<String>,
    pub provider_response: Option<Value>,
    pub stop_reason: Option<String>,
    pub cost: Option<ModelCost>,
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
            started_at: Clock::operation_snapshot().recorded(),
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
        if self.completed_at.is_empty() {
            self.completed_at = Clock::operation_snapshot().recorded();
        }
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
    network: crate::network::NetworkClients,
}
impl CallExecutor {
    pub fn new(network: crate::network::NetworkClients) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::builder()
                .with_reqwest(network.http().clone())
                .build()?,
            network,
        })
    }
    /// Exactly one external attempt. ModelCalls owns reservations and retry policy.
    pub async fn invoke<M: CallMetadata>(
        &self,
        prepared: PreparedCall<M>,
    ) -> ModelExecution<M, ProviderOutput> {
        let pricing = prepared.metadata.pricing_snapshot();
        let kind = match &prepared.request {
            PreparedRequest::Chat { .. } => UsageKind::Chat,
            PreparedRequest::Embedding { .. } => UsageKind::Embedding,
        };
        let mut record = ModelCallRecord::started(prepared.metadata);
        let result = match prepared.request {
            PreparedRequest::Chat { request, options } => {
                self.chat(prepared.target, request, *options, &mut record)
                    .await
            }
            PreparedRequest::Embedding {
                request,
                options,
                contract,
            } => super::openai_embedding::execute(
                &self.network,
                prepared.target,
                request,
                *options,
                &mut record,
                &contract,
            )
            .await
            .map(ProviderOutput::Vectors),
        };
        if record.usage.is_none() {
            record.usage =
                super::accounting::normalize_usage(kind, None, record.provider_response.as_ref());
        }
        record.cost = Some(super::accounting::cost(
            kind,
            record.usage.as_ref(),
            pricing,
            record.provider_response.as_ref(),
        ));
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
                record.usage = super::accounting::normalize_usage(
                    UsageKind::Chat,
                    Some(&response.usage),
                    response.captured_raw_body.as_ref(),
                );
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
