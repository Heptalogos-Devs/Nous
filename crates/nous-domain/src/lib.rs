//! Transport-independent identities, authority objects and access rules.

use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    InvalidArgument,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    CapabilityUnavailable,
    ResourceExhausted,
    DependencyUnavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorCode::DependencyUnavailable, error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::internal(error.to_string())
    }
}

macro_rules! identity {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                let id = uuid::Uuid::parse_str(value)
                    .map_err(|_| Error::invalid(concat!(stringify!($name), " must be a UUID")))?;
                Ok(Self(id.to_string()))
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                String::deserialize(deserializer)?
                    .parse()
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}

identity!(NamespaceId);
identity!(SubjectId);
identity!(MaterialId);
identity!(ObservationId);
identity!(MemoryId);
identity!(ModelCallId);

mod cognition;
pub use cognition::*;
mod maintenance;
pub use maintenance::*;
mod embedding;
pub use embedding::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Memory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {
    pub id: NamespaceId,
    pub name: String,
    pub revision: i64,
    pub commit_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subject {
    pub id: SubjectId,
    pub namespace_id: NamespaceId,
    pub name: String,
    pub capabilities: Vec<Capability>,
    pub revision: i64,
    pub commit_seq: i64,
}

/// Coordinate precision and calendar are owned by the named time domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimePoint {
    pub domain: String,
    pub coordinate: i64,
}

impl TimePoint {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.domain, "time domain")
    }
    pub fn compare(&self, other: &Self) -> Result<std::cmp::Ordering> {
        if self.domain != other.domain {
            return Err(Error::invalid("time points require a common time domain"));
        }
        Ok(self.coordinate.cmp(&other.coordinate))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    pub id: MaterialId,
    pub namespace_id: NamespaceId,
    pub content_hash: String,
    pub media_type: String,
    pub byte_len: i64,
    pub source: Option<String>,
    pub event_time: Option<TimePoint>,
    pub recorded_at: String,
    pub commit_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: ObservationId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub material_id: MaterialId,
    pub observed_at: Option<TimePoint>,
    pub recorded_at: String,
    pub commit_seq: i64,
    #[serde(default)]
    pub kind: ObservationKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialContent {
    pub material: Material,
    pub text: String,
    #[serde(default)]
    pub details: MaterialDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccessScope {
    Admin,
    Namespace {
        namespace_id: NamespaceId,
    },
    Subject {
        namespace_id: NamespaceId,
        subject_id: SubjectId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallContext {
    pub caller_id: String,
    pub scope: AccessScope,
}

impl CallContext {
    pub fn admin() -> Self {
        Self {
            caller_id: "admin".into(),
            scope: AccessScope::Admin,
        }
    }
    pub fn require_admin(&self) -> Result<()> {
        match self.scope {
            AccessScope::Admin => Ok(()),
            _ => Err(Error::new(
                ErrorCode::Forbidden,
                "administrator scope required",
            )),
        }
    }
    pub fn require_namespace(&self, namespace: &NamespaceId) -> Result<()> {
        match &self.scope {
            AccessScope::Admin => Ok(()),
            AccessScope::Namespace { namespace_id } if namespace_id == namespace => Ok(()),
            _ => Err(Error::new(ErrorCode::Forbidden, "namespace scope required")),
        }
    }
    pub fn require_subject(&self, namespace: &NamespaceId, subject: &SubjectId) -> Result<()> {
        match &self.scope {
            AccessScope::Admin => Ok(()),
            AccessScope::Namespace { namespace_id } if namespace_id == namespace => Ok(()),
            AccessScope::Subject {
                namespace_id,
                subject_id,
            } if namespace_id == namespace && subject_id == subject => Ok(()),
            _ => Err(Error::new(ErrorCode::Forbidden, "subject scope required")),
        }
    }
}

pub fn validate_name(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(Error::invalid(format!(
            "{field} must contain 1 to 256 bytes without control characters"
        )));
    }
    Ok(())
}
