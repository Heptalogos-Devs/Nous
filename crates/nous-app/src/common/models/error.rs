use nous_domain::{Error, ErrorCode};
use serde::Serialize;
use std::{error::Error as StdError, fmt};

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

/// Typed causes stay inspectable in process; only the redacted info is persisted.
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
    pub(crate) fn new(class: RetryClass, code: ErrorCode, message: impl Into<String>) -> Self {
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
    pub(crate) fn waiting(message: impl Into<String>) -> Self {
        Self::new(
            RetryClass::WaitingConfiguration,
            ErrorCode::CapabilityUnavailable,
            message,
        )
    }
    pub(crate) fn prompt(error: Error) -> Self {
        let class = if error.code == ErrorCode::ResourceExhausted {
            RetryClass::Permanent
        } else {
            RetryClass::WaitingConfiguration
        };
        Self::new(class, error.code, error.message)
    }
    pub(crate) fn domain(error: Error) -> Self {
        Self::new(RetryClass::Permanent, error.code, error.message)
    }
    pub(crate) fn with_source(mut self, source: Box<dyn StdError + Send + Sync>) -> Self {
        self.source = Some(source);
        self
    }
    pub(crate) fn provider(error: genai::Error) -> Self {
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
            || "model request failed; typed provider cause retained".into(),
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.info.fmt(f)
    }
}
impl fmt::Display for ModelFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.info.message.fmt(f)
    }
}
impl StdError for ModelFailure {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn StdError + 'static))
    }
}
