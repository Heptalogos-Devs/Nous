//! Complete OpenAI embedding exchange, with vector validity separate from accounting.
use super::{ModelCallRecord, ModelFailure, RetryClass, accounting};
use crate::network::NetworkClients;
use genai::{
    ServiceTarget,
    embed::{EmbedInput, EmbedOptions, EmbedRequest},
};
use nous_domain::{EmbeddingSpace, Error, ErrorCode, UsageKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone)]
pub(super) struct ResponseContract {
    pub space: EmbeddingSpace,
    pub inputs: usize,
    pub max_bytes: usize,
}
#[derive(Serialize)]
struct Request {
    input: Input,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Input {
    Single(String),
    Batch(Vec<String>),
}
#[derive(Deserialize)]
struct Response {
    data: Vec<Embedding>,
    model: String,
}
#[derive(Deserialize)]
struct Embedding {
    embedding: Vec<f32>,
    index: usize,
}

pub(super) async fn execute<M>(
    network: &NetworkClients,
    target: ServiceTarget,
    request: EmbedRequest,
    options: EmbedOptions,
    record: &mut ModelCallRecord<M>,
    contract: &ResponseContract,
) -> Result<Vec<Vec<f32>>, ModelFailure> {
    let api_key = target.auth.single_key_value().map_err(|error| {
        ModelFailure::waiting("embedding provider credential is unavailable")
            .with_source(Box::new(error))
    })?;
    let base = reqwest::Url::parse(target.endpoint.base_url()).map_err(|error| {
        ModelFailure::domain(Error::invalid("embedding provider endpoint is invalid"))
            .with_source(Box::new(error))
    })?;
    let mut url = base.join("embeddings").map_err(|error| {
        ModelFailure::domain(Error::invalid(
            "embedding provider endpoint cannot address embeddings",
        ))
        .with_source(Box::new(error))
    })?;
    url.set_query(base.query());
    let (_, model) = target.model.model_name.namespace_and_name();
    let payload = Request {
        model: model.into(),
        input: match request.input {
            EmbedInput::Single(text) => Input::Single(text),
            EmbedInput::Batch(texts) => Input::Batch(texts),
        },
        encoding_format: options.encoding_format.clone(),
        dimensions: options.dimensions,
        user: options.user.clone(),
    };
    let mut outgoing = network
        .request(reqwest::Method::POST, url.as_str())
        .bearer_auth(api_key)
        .json(&payload);
    if let Some(headers) = &options.headers {
        for (name, value) in headers {
            outgoing = outgoing.header(name.as_str(), value.as_str());
        }
    }
    let response = network.send(outgoing).await.map_err(transport)?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let bytes = network
        .read_bytes(response, contract.max_bytes)
        .await
        .map_err(|error| {
            let class = if error.code == ErrorCode::ResourceExhausted {
                RetryClass::Permanent
            } else {
                RetryClass::Uncertain
            };
            let mut failure = ModelFailure::new(
                class,
                error.code,
                "embedding response could not be fully read",
            );
            failure.info.error = error;
            failure
        })?;
    record.raw_output = Some(String::from_utf8_lossy(&bytes).into_owned());
    let json: Result<Value, _> = serde_json::from_slice(&bytes);
    match json {
        Ok(body) => {
            record.provider_model = body.get("model").and_then(Value::as_str).map(str::to_owned);
            record.usage = accounting::normalize_usage(UsageKind::Embedding, None, Some(&body));
            record.provider_response = Some(body);
        }
        Err(error) => {
            use base64::Engine;
            record.provider_response = Some(
                serde_json::json!({"raw_body_base64":base64::engine::general_purpose::STANDARD.encode(&bytes)}),
            );
            if !status.is_success() {
                return Err(http_failure(status.as_u16(), retry_after));
            }
            return Err(
                invalid("embedding provider response must be JSON").with_source(Box::new(error))
            );
        }
    }
    if !status.is_success() {
        return Err(http_failure(status.as_u16(), retry_after));
    }
    let response: Response = serde_json::from_value(
        record
            .provider_response
            .as_ref()
            .expect("captured JSON")
            .clone(),
    )
    .map_err(|error| {
        invalid("embedding response does not match the vector contract")
            .with_source(Box::new(error))
    })?;
    validate(response, &contract.space, contract.inputs)
}
fn validate(
    response: Response,
    space: &EmbeddingSpace,
    count: usize,
) -> Result<Vec<Vec<f32>>, ModelFailure> {
    if response.model != space.provider_model {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::Conflict,
            "embedding provider model differs from the configured immutable space",
        ));
    }
    if response.data.len() != count {
        return Err(invalid(
            "embedding response must cover every input exactly once",
        ));
    }
    let mut ordered = vec![None; count];
    for item in response.data {
        if item.index >= count || ordered[item.index].is_some() {
            return Err(invalid(
                "embedding response contains a duplicate or out-of-range index",
            ));
        }
        if item.embedding.len() != space.dimension
            || !item.embedding.iter().all(|value| value.is_finite())
            || !item.embedding.iter().any(|value| *value != 0.0)
        {
            return Err(invalid(
                "embedding response must contain finite nonzero vectors with the configured dimension",
            ));
        }
        ordered[item.index] = Some(item.embedding);
    }
    ordered
        .into_iter()
        .map(|vector| vector.ok_or_else(|| invalid("embedding response is missing an input index")))
        .collect()
}
fn invalid(message: &str) -> ModelFailure {
    ModelFailure::new(RetryClass::Permanent, ErrorCode::InvalidArgument, message)
}
fn transport(error: reqwest::Error) -> ModelFailure {
    let class = if error.is_builder() {
        RetryClass::Permanent
    } else if error.is_connect() {
        RetryClass::Retryable
    } else {
        RetryClass::Uncertain
    };
    let message = if error.is_connect() {
        "cannot connect to embedding provider"
    } else if error.is_timeout() {
        "embedding request timed out"
    } else {
        "embedding request failed"
    };
    ModelFailure::new(class, ErrorCode::DependencyUnavailable, message)
        .with_source(Box::new(error.without_url()))
}
fn http_failure(status: u16, retry_after: Option<u64>) -> ModelFailure {
    let class = match status {
        408 => RetryClass::Uncertain,
        429 | 500..=599 => RetryClass::Retryable,
        _ => RetryClass::Permanent,
    };
    let mut failure = ModelFailure::new(
        class,
        ErrorCode::DependencyUnavailable,
        format!("embedding provider returned HTTP {status}"),
    );
    failure.info.http_status = Some(status);
    failure.info.retry_after_seconds = retry_after;
    failure
}
