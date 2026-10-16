//! Evidence and material semantics.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use nous_core::{
    ArtifactId, DerivedRegionId, DerivedRepresentationId, EntityRef, Error, ObjectRef,
    OccurrenceId, ProducerSignature, RepresentationKind, ResourceRef, Result, SourceClass,
    SourceRegionId, SubjectId, TemporalExtent,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use nous_core::{AuthorityClass, Modality};

pub fn media_modality(media: &str) -> Modality {
    if media.starts_with("text/") {
        Modality::Text
    } else if media.starts_with("image/") {
        Modality::Image
    } else if media.starts_with("audio/") {
        Modality::Audio
    } else if media.starts_with("video/") {
        Modality::Video
    } else if media.contains("json") || media.contains("xml") {
        Modality::Structured
    } else {
        Modality::Binary
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: ArtifactId,
    pub subject_id: SubjectId,
    pub content_hash: String,
    pub byte_length: u64,
    pub media_type: String,
    pub created_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationOccurrence {
    pub occurrence_id: OccurrenceId,
    pub subject_id: SubjectId,
    pub artifact_id: Option<ArtifactId>,
    pub source_class: SourceClass,
    pub external_object_ref: Option<ObjectRef>,
    pub occurred_time: TemporalExtent,
    pub occurred_time_domain_ref: String,
    pub observed_at: DateTime<Utc>,
    pub observed_stamp: nous_core::DomainStamp,
    pub conversation_ref: Option<String>,
    pub actor_entity_ref: Option<EntityRef>,
    pub context: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl ObservationOccurrence {
    pub fn validate(&self) -> Result<()> {
        nous_core::DomainTemporalExtent {
            time_domain_ref: self.occurred_time_domain_ref.clone(),
            extent: self.occurred_time.clone(),
        }
        .validate()?;
        if self.observed_at < DateTime::<Utc>::UNIX_EPOCH {
            return Err(Error::Invalid("observed_at is before Unix epoch".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRegion {
    pub source_region_id: SourceRegionId,
    pub subject_id: SubjectId,
    pub artifact_id: ArtifactId,
    pub coordinate_kind: String,
    pub coordinate: serde_json::Value,
    pub coordinate_hash: String,
    pub parent_source_region_id: Option<SourceRegionId>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivationInput {
    pub ordinal: u32,
    pub reference: nous_core::CognitiveRef,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedRepresentation {
    pub formed_stamp: Option<nous_core::DomainStamp>,
    pub derived_representation_id: DerivedRepresentationId,
    pub subject_id: SubjectId,
    pub inputs: Vec<DerivationInput>,
    pub strategy: String,
    pub representation_kind: RepresentationKind,
    pub producer: ProducerSignature,
    pub revision: i32,
    pub payload_text: Option<String>,
    pub payload_json: Option<serde_json::Value>,
    pub payload_artifact_id: Option<ArtifactId>,
    pub quality: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub supersedes: Option<DerivedRepresentationId>,
}

impl DerivedRepresentation {
    pub fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        if self.inputs.is_empty() || self.inputs.len() > 64 || self.strategy.trim().is_empty() {
            return Err(Error::Invalid(
                "derivation requires 1..64 ordered inputs and a strategy".into(),
            ));
        }
        for (ordinal, input) in self.inputs.iter().enumerate() {
            if input.ordinal as usize != ordinal
                || input.role.trim().is_empty()
                || !matches!(
                    input.reference,
                    nous_core::CognitiveRef::SourceRegion(_)
                        | nous_core::CognitiveRef::DerivedRepresentation(_)
                        | nous_core::CognitiveRef::DerivedRegion(_)
                )
                || !seen.insert(&input.reference)
                || input.reference
                    == nous_core::CognitiveRef::DerivedRepresentation(
                        self.derived_representation_id,
                    )
            {
                return Err(Error::Invalid(
                    "invalid, duplicate, or cyclic derivation input".into(),
                ));
            }
        }
        if self.payload_text.is_none()
            && self.payload_json.is_none()
            && self.payload_artifact_id.is_none()
        {
            return Err(Error::Invalid(
                "derived representation needs text, structured JSON or artifact payload".into(),
            ));
        }
        if self
            .payload_json
            .as_ref()
            .is_some_and(|value| !value.is_object() || value.to_string().len() > 262_144)
        {
            return Err(Error::Invalid("invalid structured payload bounds".into()));
        }
        if self.revision < 1 {
            return Err(Error::Invalid(
                "derived representation revision starts at 1".into(),
            ));
        }
        if self.representation_kind == RepresentationKind::StructuredInterpretation
            && (self.payload_json.is_none() || self.producer.output_schema_digest.is_none())
        {
            return Err(Error::Invalid(
                "structured interpretation requires payload and schema identity".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedRegion {
    pub derived_region_id: DerivedRegionId,
    pub subject_id: SubjectId,
    pub derived_representation_id: DerivedRepresentationId,
    pub coordinate_kind: String,
    pub coordinate: serde_json::Value,
    pub coordinate_hash: String,
    pub parent_derived_region_id: Option<DerivedRegionId>,
    pub created_at: DateTime<Utc>,
    pub formed_stamp: Option<nous_core::DomainStamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservationMaterial {
    InlineText { text: String, media_type: String },
    ArtifactRef { artifact_id: ArtifactId },
    ExternalObjectRef { object_ref: ObjectRef },
    StructuredJson { value: serde_json::Value },
    ResourceAvailability { resource: ResourceRef },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccurrenceDescriptor {
    pub source_class: SourceClass,
    pub external_object_ref: Option<ObjectRef>,
    pub occurred_time: TemporalExtent,
    #[serde(
        default = "real_time_domain",
        skip_serializing_if = "is_real_time_domain"
    )]
    pub occurred_time_domain_ref: String,
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    pub conversation_ref: Option<String>,
    pub actor_entity_ref: Option<EntityRef>,
    #[serde(default)]
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDirective {
    #[serde(default = "default_true")]
    pub admit: bool,
    pub hold_until: Option<DateTime<Utc>>,
}

impl Default for RuntimeDirective {
    fn default() -> Self {
        Self {
            admit: true,
            hold_until: None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEntityMention {
    pub surface: String,
    pub entity_ref: Option<EntityRef>,
    pub semantic_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationInput {
    #[serde(default)]
    pub subject: SubjectId,
    pub session: Option<nous_core::SessionId>,
    pub occurrence: OccurrenceDescriptor,
    pub material: ObservationMaterial,
    #[serde(default)]
    pub entities: Vec<ResolvedEntityMention>,
    #[serde(default)]
    pub runtime: RuntimeDirective,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptedObservation {
    pub artifact: Option<Artifact>,
    pub occurrence: ObservationOccurrence,
    pub source_region: Option<SourceRegion>,
    #[serde(default)]
    pub mention_ids: Vec<Uuid>,
    pub resident: bool,
    pub memory_revisions: Vec<nous_core::MemoryRevisionId>,
}

fn real_time_domain() -> String {
    "time:real".into()
}
fn is_real_time_domain(value: &String) -> bool {
    value == "time:real"
}
