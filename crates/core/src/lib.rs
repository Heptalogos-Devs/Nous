//! Shared semantic primitives for Nous Wave.
//!
//! This crate deliberately contains no SQL, HTTP, model-provider, vector-index,
//! or object-store mechanics. It is the vocabulary shared by Authority,
//! runtime, projections, and the host-facing API.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod time;
pub use time::{TemporalExtent, TimeInterval, TimePredicate};
mod model_results;
pub use model_results::{
    AttemptStatus, ExecutionAttempt, ExecutionTelemetry, ExecutionUsage, MaintenanceTrigger,
    ModelResultContext, ResultPayload,
};
mod topology;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
pub use topology::TopologyRelation;
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

uuid_id!(SubjectId);
uuid_id!(SelfFacetId);
uuid_id!(SelfFacetRevisionId);
uuid_id!(NarrativeThreadId);
uuid_id!(NarrativeThreadRevisionId);
uuid_id!(MemoryId);
uuid_id!(MemoryRevisionId);
uuid_id!(EpisodeId);
uuid_id!(EpisodeRevisionId);
uuid_id!(JournalId);
uuid_id!(JournalRevisionId);
uuid_id!(CognitiveSchemaId);
uuid_id!(CognitiveSchemaRevisionId);
uuid_id!(SchemaEvidenceLinkId);
uuid_id!(AssociationEvidenceId);
uuid_id!(ArtifactId);
uuid_id!(OccurrenceId);
uuid_id!(SourceRegionId);
uuid_id!(DerivedRepresentationId);
uuid_id!(DerivedRegionId);
uuid_id!(TagId);
uuid_id!(SessionId);
uuid_id!(WorkContextId);
uuid_id!(DerivationId);
uuid_id!(ServingGenerationId);
uuid_id!(UseEventId);
uuid_id!(OperationId);
uuid_id!(CognitiveSeedVersionId);

macro_rules! opaque_ref {
    ($(#[$metadata:meta])* $name:ident, $prefix:literal) => {
        $(#[$metadata])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                validate_opaque_ref(&value, $prefix)?;
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> { Self::new(value) }
        }
    };
}
opaque_ref!(
    /// A Host-owned identity, opaque except for exact equality and namespace.
    EntityRef, "entity"
);
opaque_ref!(
    /// A Host-owned resource identity; contents retain external Authority.
    ResourceRef, "resource"
);
opaque_ref!(
    /// An opaque reference to an external object owned by a Host system.
    ObjectRef, "object"
);
fn validate_opaque_ref(value: &str, expected_prefix: &str) -> Result<()> {
    let parts: Vec<_> = value.split(':').collect();
    if value.is_empty()
        || value.len() > 1024
        || parts.len() < 2
        || parts[0] != expected_prefix
        || value.chars().any(char::is_whitespace)
        || parts.iter().any(|part| part.is_empty())
    {
        return Err(Error::Invalid(format!(
            "invalid {expected_prefix} reference"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityClass {
    SubjectCognition,
    ExternalCurrentAuthority,
    Evidence,
    Interpretation,
    ResourceDescriptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicClass {
    Observed,
    Reported,
    Derived,
    Inferred,
    Narrative,
    Simulated,
}

impl EpistemicClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Reported => "reported",
            Self::Derived => "derived",
            Self::Inferred => "inferred",
            Self::Narrative => "narrative",
            Self::Simulated => "simulated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum SourceClass {
    Message,
    Tool,
    File,
    Web,
    HostEvent,
    Simulation,
    Generated,
    Import,
    Other(String),
}

impl SourceClass {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Message => "message",
            Self::Tool => "tool",
            Self::File => "file",
            Self::Web => "web",
            Self::HostEvent => "host_event",
            Self::Simulation => "simulation",
            Self::Generated => "generated",
            Self::Import => "import",
            Self::Other(value) => value,
        }
    }
}

impl From<String> for SourceClass {
    fn from(value: String) -> Self {
        match value.as_str() {
            "message" => Self::Message,
            "tool" => Self::Tool,
            "file" => Self::File,
            "web" => Self::Web,
            "host_event" => Self::HostEvent,
            "simulation" => Self::Simulation,
            "generated" => Self::Generated,
            "import" => Self::Import,
            _ => Self::Other(value),
        }
    }
}

impl From<SourceClass> for String {
    fn from(value: SourceClass) -> Self {
        value.as_str().to_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOperation {
    TextEmbedding,
    TextRerank,
    TextInterpretation,
    MemoryFormationText,
    SelfFormationText,
    SelfReflectionText,
    MemoryConsolidationText,
    ConceptMaintenanceText,
    EpisodeSegmentationText,
    JournalSynthesisText,
    ImageInterpretation,
    ImageEmbedding,
    SpeechTranscription,
    DocumentExtraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
    Audio,
    Video,
    Structured,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationKind {
    ExtractedText,
    Ocr,
    Transcript,
    AudioDescription,
    ImageDescription,
    SceneDescription,
    Summary,
    StructuredInterpretation,
    ResourceSynopsis,
    Other,
}

impl RepresentationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExtractedText => "extracted_text",
            Self::Ocr => "ocr",
            Self::Transcript => "transcript",
            Self::AudioDescription => "audio_description",
            Self::ImageDescription => "image_description",
            Self::SceneDescription => "scene_description",
            Self::Summary => "summary",
            Self::StructuredInterpretation => "structured_interpretation",
            Self::ResourceSynopsis => "resource_synopsis",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStrength {
    Required,
    Preferred,
    #[default]
    Optional,
    Forbidden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProducerSignature {
    pub signature_hash: String,
    pub provider_class: String,
    pub operation: CapabilityOperation,
    pub implementation: String,
    pub model_identity: Option<String>,
    pub model_revision: Option<String>,
    pub output_schema_digest: Option<String>,
    pub preprocessing_identity: String,
    pub preprocessing_revision: String,
    pub config_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_controls_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_policy_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingSpaceSignature {
    pub space_hash: String,
    pub model_identity: String,
    pub weights_revision: String,
    pub task: String,
    pub input_representation: String,
    pub preprocessing_identity: String,
    pub preprocessing_revision: String,
    pub dimension: u32,
    pub normalization: String,
    pub output_semantics: String,
}

impl EmbeddingSpaceSignature {
    pub fn compatible_with(&self, other: &Self) -> bool {
        self == other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessDescriptor {
    #[serde(default)]
    pub occurred: Vec<TemporalExtent>,
    pub observed_at: Option<DateTime<Utc>>,
    pub valid_time: TemporalExtent,
    pub formed_at: Option<DateTime<Utc>>,
    pub recorded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceHandle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epistemic_relation: Option<EpistemicRelation>,
    pub reference: CognitiveRef,
    pub basis_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializationHandle {
    pub reference: CognitiveRef,
    pub level: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SituationDescriptor {
    #[serde(default)]
    pub object_descriptions: std::collections::BTreeMap<String, String>,
    pub consumer: Option<String>,
    #[serde(default)]
    pub current_refs: Vec<CognitiveRef>,
    #[serde(default)]
    pub current_objects: Vec<ObjectRef>,
}

mod error;
pub use error::{DomainError, DomainErrorCode, Error, Result};

mod canonical;
pub mod cognition;
mod concept;
mod history;
pub use history::*;
mod query;
pub use concept::*;
mod references;
pub use canonical::canonical_request_digest;
pub use cognition::*;
pub use query::*;
pub use references::{CognitiveDomain, CognitiveRef, parse_reference, reference_parts};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseEventRef {
    pub subject_id: SubjectId,
    pub consumer_ref: String,
    pub event_id: UseEventId,
}

impl UseEventRef {
    pub fn canonical_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.subject_id.0, self.consumer_ref, self.event_id.0
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryDescriptor {
    pub reference: CognitiveRef,
    pub text: String,
}
