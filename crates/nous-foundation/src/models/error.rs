use nous_domain::{Error, ErrorCause, ErrorCode};
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
    pub http_status: Option<u16>,
    pub retry_after_seconds: Option<u64>,
    pub error: Error,
}

/// SDK errors stay inspectable in process; persisted diagnostics omit request URLs.
pub struct ModelFailure {
    pub info: Box<ModelFailureInfo>,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl ModelFailure {
    pub fn class(&self) -> RetryClass {
        self.info.class
    }
    pub fn to_domain_error(&self) -> Error {
        self.info.error.clone()
    }
    pub fn new(class: RetryClass, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            info: Box::new(ModelFailureInfo {
                class,
                http_status: None,
                retry_after_seconds: None,
                error: Error::new(code, message),
            }),
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
        Self::from_domain(class, error)
    }
    pub fn domain(error: Error) -> Self {
        Self::from_domain(RetryClass::Permanent, error)
    }
    fn from_domain(class: RetryClass, error: Error) -> Self {
        Self {
            info: Box::new(ModelFailureInfo {
                class,
                http_status: None,
                retry_after_seconds: None,
                error,
            }),
            source: None,
        }
    }
    pub fn with_source(mut self, source: Box<dyn StdError + Send + Sync>) -> Self {
        self.info.error = self.info.error.clone().with_cause(source.as_ref());
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
        let transport = match &error {
            genai::Error::WebModelCall {
                webc_error: genai::webc::Error::Reqwest(error),
                ..
            }
            | genai::Error::WebAdapterCall {
                webc_error: genai::webc::Error::Reqwest(error),
                ..
            } => Some(error),
            _ => None,
        };
        let cause = transport.and_then(|error| {
            let mut source = error.source()?;
            while let Some(inner) = source.source() {
                source = inner;
            }
            Some(source.to_string().chars().take(1024).collect())
        });
        let message = if let Some(status) = status {
            format!("model provider returned HTTP {status}")
        } else if transport.is_some_and(|error| error.is_connect()) {
            "cannot connect to model provider".into()
        } else if transport.is_some_and(|error| error.is_timeout()) {
            "model provider request timed out".into()
        } else if transport.is_some_and(|error| error.is_builder()) {
            "cannot construct model provider request".into()
        } else {
            "model provider request failed".into()
        };
        let mut diagnostic = Error::new(ErrorCode::DependencyUnavailable, message);
        diagnostic.cause = cause.map(|message| ErrorCause {
            message,
            source: None,
        });
        Self {
            info: Box::new(ModelFailureInfo {
                class,
                http_status: status,
                retry_after_seconds,
                error: diagnostic,
            }),
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
        self.info.error.message.fmt(f)
    }
}
impl StdError for ModelFailure {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn StdError + 'static))
    }
}
