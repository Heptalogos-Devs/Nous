use super::{ModelCallRecord, ModelFailure, RetryClass, accounting};
use crate::network::NetworkClients;
use genai::ServiceTarget;
use nous_domain::{Error, ErrorCode, UsageKind};
use serde::Serialize;
use serde_json::Value;

pub(super) async fn exchange<M>(
    network: &NetworkClients,
    target: ServiceTarget,
    suffix: &str,
    payload: &impl Serialize,
    record: &mut ModelCallRecord<M>,
    max_bytes: usize,
    usage_kind: UsageKind,
) -> Result<Value, ModelFailure> {
    let api_key = target.auth.single_key_value().map_err(|error| {
        ModelFailure::waiting("model provider credential is unavailable")
            .with_source(Box::new(error))
    })?;
    let mut url = reqwest::Url::parse(target.endpoint.base_url()).map_err(|error| {
        ModelFailure::domain(Error::invalid("model provider endpoint is invalid"))
            .with_source(Box::new(error))
    })?;
    let path = format!("{}/{}", url.path().trim_end_matches('/'), suffix);
    url.set_path(&path);
    let outgoing = network
        .request(reqwest::Method::POST, url.as_str())
        .bearer_auth(api_key)
        .json(payload);
    let response = network.send(outgoing).await.map_err(transport)?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let bytes = network
        .read_bytes(response, max_bytes)
        .await
        .map_err(|error| {
            let class = if error.code == ErrorCode::ResourceExhausted {
                RetryClass::Permanent
            } else {
                RetryClass::Uncertain
            };
            let mut failure =
                ModelFailure::new(class, error.code, "model response could not be fully read");
            failure.info.error = error;
            failure
        })?;
    record.raw_output = Some(String::from_utf8_lossy(&bytes).into_owned());
    let json: Result<Value, _> = serde_json::from_slice(&bytes);
    match json {
        Ok(body) => {
            record.provider_model = body.get("model").and_then(Value::as_str).map(str::to_owned);
            record.usage = accounting::normalize_usage(usage_kind, None, Some(&body));
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
            return Err(ModelFailure::domain(Error::invalid(
                "model provider response must be JSON",
            ))
            .with_source(Box::new(error)));
        }
    }
    if !status.is_success() {
        return Err(http_failure(status.as_u16(), retry_after));
    }
    Ok(record
        .provider_response
        .as_ref()
        .expect("captured provider response")
        .clone())
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
        "cannot connect to model provider"
    } else if error.is_timeout() {
        "model request timed out"
    } else {
        "model request failed"
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
        format!("model provider returned HTTP {status}"),
    );
    failure.info.http_status = Some(status);
    failure.info.retry_after_seconds = retry_after;
    failure
}
