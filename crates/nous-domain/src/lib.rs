//! Transport-independent identities, authority objects and access rules.

use serde::{Deserialize, Serialize};

mod error;
pub use error::*;
mod context;
pub use context::*;

macro_rules! identity {
    ($name:ident) => {
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, schemars::JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn validate(&self) -> $crate::Result<()> {
                $crate::identity::validate_uuid(&self.0)
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl std::str::FromStr for $name {
            type Err = $crate::Error;
            fn from_str(value: &str) -> $crate::Result<Self> {
                let id = $crate::identity::parse_uuid(value)?;
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
identity!(CognitiveId);
identity!(TraceId);

mod configuration;
mod identity;
pub mod media;
pub use configuration::*;

mod time;
pub use time::*;
mod query;
pub use query::*;
mod cognition;
pub use cognition::*;
mod model;
pub use model::*;
mod model_budget;
pub use model_budget::*;
mod task;
pub use task::*;
mod maintenance;
pub use maintenance::*;
mod embedding;
pub use embedding::*;
mod structured;
pub use structured::*;

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

pub fn validate_name(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(Error::invalid(format!(
            "{field} must contain 1 to 256 bytes without control characters"
        )));
    }
    Ok(())
}

mod prompt;
pub use prompt::{PromptRef, PromptRevision, validate_prompt_body, validate_prompt_locale};
