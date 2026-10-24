//! Serializable failures retain their cause chain and operation context.
use crate::TraceId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    InvalidArgument,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Cancelled,
    CapabilityUnavailable,
    ResourceExhausted,
    DependencyUnavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ErrorCause {
    pub message: String,
    #[source]
    pub source: Option<Box<ErrorCause>>,
}

impl ErrorCause {
    fn capture(error: &(dyn std::error::Error + 'static), remaining: usize) -> Self {
        Self {
            message: error.to_string(),
            source: if remaining == 0 {
                None
            } else {
                error
                    .source()
                    .map(|source| Box::new(Self::capture(source, remaining - 1)))
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    pub trace_id: Option<TraceId>,
    pub context: BTreeMap<String, Value>,
    #[source]
    pub cause: Option<ErrorCause>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            trace_id: None,
            context: BTreeMap::new(),
            cause: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
    pub fn dependency(source: impl std::error::Error + 'static) -> Self {
        Self::new(ErrorCode::DependencyUnavailable, source.to_string()).with_cause(&source)
    }
    pub fn with_trace(mut self, trace_id: TraceId) -> Self {
        self.trace_id = Some(trace_id);
        self
    }
    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
    pub fn with_cause(mut self, cause: &(dyn std::error::Error + 'static)) -> Self {
        self.cause = Some(ErrorCause::capture(cause, 32));
        self
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorCode::DependencyUnavailable, error.to_string()).with_cause(&error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::internal(error.to_string()).with_cause(&error)
    }
}
