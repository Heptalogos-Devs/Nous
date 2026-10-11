//! Evidence and material semantics.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use nous_core::{
    ArtifactId, CapabilityOperation, DerivedRegionId, DerivedRepresentationId, EntityRef, Error,
    ObjectRef, OccurrenceId, ProducerSignature, RepresentationKind, ResourceRef, Result,
    SourceClass, SourceRegionId, SubjectId, TemporalExtent,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use nous_core::{AuthorityClass, Modality};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: ArtifactId,
    pub subject_id: SubjectId,
    pub content_hash: String,
    pub byte_length: u64,
    pub media_type: String,
    pub storage_key: String,
    pub created_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

impl Artifact {
    pub fn validate(&self) -> Result<()> {
        if self.content_hash.len() != 64
            || !self
                .content_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "artifact content hash must be BLAKE3 hex",
            ));
        }
        if self.media_type.trim().is_empty() || self.storage_key.trim().is_empty() {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "artifact media_type/storage_key is required",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationOccurrence {
    pub occurrence_id: OccurrenceId,
    pub subject_id: SubjectId,
    pub artifact_id: Option<ArtifactId>,
    pub source_class: SourceClass,
    pub external_object_ref: Option<ObjectRef>,
    pub occurred_time: TemporalExtent,
    pub observed_at: DateTime<Utc>,
    pub conversation_ref: Option<String>,
    pub actor_entity_ref: Option<EntityRef>,
    pub context: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl ObservationOccurrence {
    pub fn validate(&self) -> Result<()> {
        if self.observed_at < DateTime::<Utc>::UNIX_EPOCH {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "observed_at is before Unix epoch",
            ));
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

impl SourceRegion {
    pub fn validate(&self) -> Result<()> {
        const KINDS: &[&str] = &[
            "whole_artifact",
            "byte_range",
            "text_span",
            "pdf_region",
            "image_bbox",
            "media_time",
            "json_pointer",
            "structural_path",
        ];
        if !KINDS.contains(&self.coordinate_kind.as_str()) {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "unsupported source coordinate kind",
            ));
        }
        if self.coordinate_hash.is_empty() {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "source coordinate hash is required",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivationInput {
    pub ordinal: u32,
    pub reference: nous_core::CognitiveRef,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedRepresentation {
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
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "derivation requires 1..64 ordered inputs and a strategy",
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
                || !seen.insert(input.reference.to_string())
                || input.reference
                    == nous_core::CognitiveRef::DerivedRepresentation(
                        self.derived_representation_id,
                    )
            {
                return Err(Error::new(
                    nous_core::ErrorOwner::Material,
                    nous_core::ErrorCode::InvalidRequest,
                    "invalid, duplicate, or cyclic derivation input",
                ));
            }
        }
        if self.payload_text.is_none()
            && self.payload_json.is_none()
            && self.payload_artifact_id.is_none()
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "derived representation needs text, structured JSON or artifact payload",
            ));
        }
        if self
            .payload_json
            .as_ref()
            .is_some_and(|value| !value.is_object() || value.to_string().len() > 262_144)
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "invalid structured payload bounds",
            ));
        }
        if self.revision < 1 {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "derived representation revision starts at 1",
            ));
        }
        if self.representation_kind == RepresentationKind::StructuredInterpretation
            && (self.payload_json.is_none() || self.producer.output_schema_digest.is_none())
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "structured interpretation requires payload and schema identity",
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
}

impl DerivedRegion {
    pub fn validate(&self) -> Result<()> {
        const KINDS: &[&str] = &[
            "text_span",
            "segment",
            "description_segment",
            "bbox-in-derived",
            "structural_path",
        ];
        if !KINDS.contains(&self.coordinate_kind.as_str()) || self.coordinate_hash.trim().is_empty()
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "invalid derived coordinate",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageNeed {
    pub coverage_need_id: Uuid,
    pub subject_id: SubjectId,
    pub source_region_id: SourceRegionId,
    pub representation_kind: RepresentationKind,
    pub capability_operation: CapabilityOperation,
    pub requirement: String,
    pub state: String,
    pub current_representation_id: Option<DerivedRepresentationId>,
    pub updated_at: DateTime<Utc>,
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

impl ObservationMaterial {
    pub fn modality(&self) -> Modality {
        match self {
            Self::InlineText { .. } => Modality::Text,
            Self::ArtifactRef { .. } => Modality::Binary,
            Self::ExternalObjectRef { .. } => Modality::Structured,
            Self::StructuredJson { .. } | Self::ResourceAvailability { .. } => Modality::Structured,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccurrenceDescriptor {
    pub source_class: SourceClass,
    pub external_object_ref: Option<ObjectRef>,
    pub occurred_time: TemporalExtent,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolObservation {
    pub subject: SubjectId,
    pub session: Option<nous_core::SessionId>,
    pub occurred_time: TemporalExtent,
    pub observed_at: DateTime<Utc>,
    pub tool_ref: Option<ObjectRef>,
    pub result: serde_json::Value,
    #[serde(default)]
    pub context: serde_json::Value,
}

#[cfg(test)]
#[path = "../tests/unit/domain.rs"]
mod tests;
