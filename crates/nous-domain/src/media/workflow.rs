//! Accepted media work and its exact, committed preparation directory.
use super::*;
use crate::{JobRecord, ObservationKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaDelivery {
    #[default]
    Native,
    SampledFrames,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeriveMediaRequest {
    pub subject_id: SubjectId,
    pub observation_id: ObservationId,
    pub source: MediaReference,
    pub strategy: DerivationStrategy,
    #[serde(default)]
    pub delivery: MediaDelivery,
    pub revise: Option<RepresentationRef>,
}
impl DeriveMediaRequest {
    pub fn validate(&self) -> Result<()> {
        self.subject_id.validate()?;
        validate_uuid(self.observation_id.as_str())?;
        self.source.validate()?;
        if let Some(reference) = &self.revise {
            reference.id.validate()?;
            if reference.revision < 1 {
                return Err(Error::invalid(
                    "media revision requires an exact positive prior revision",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaPreparedInput {
    pub source: MediaReference,
    pub role: DerivationInputRole,
    pub at_ms: Option<u64>,
    pub text_source: Option<RepresentationRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaPreparedGraph {
    pub inputs: Vec<MediaPreparedInput>,
    pub quality: DerivationQuality,
    pub preprocessing_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaPreparationRecord {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub source: MediaReference,
    pub plan_digest: String,
    pub graph: MediaPreparedGraph,
    pub recorded_at: String,
    pub commit_seq: i64,
}
impl MediaPreparationRecord {
    pub fn validate(&self, duration_ms: Option<u64>) -> Result<()> {
        validate_uuid(self.namespace_id.as_str())?;
        self.subject_id.validate()?;
        self.source.validate()?;
        valid_digest(&self.plan_digest)?;
        self.graph.validate(duration_ms)?;
        bounded_text(&self.recorded_at, 128, "recorded_at", true)?;
        if self.commit_seq < 1 {
            return Err(Error::invalid(
                "media preparation requires a committed authority sequence",
            ));
        }
        Ok(())
    }
}
impl MediaPreparedGraph {
    pub fn validate(&self, duration_ms: Option<u64>) -> Result<()> {
        valid_digest(&self.preprocessing_digest)?;
        self.quality.validate(duration_ms)?;
        if self.inputs.is_empty() || self.inputs.len() > 64 {
            return Err(Error::invalid("prepared media requires 1..64 exact inputs"));
        }
        let mut seen = BTreeSet::new();
        for input in &self.inputs {
            input.source.validate()?;
            if let Some(reference) = &input.text_source {
                MediaReference::Representation {
                    reference: reference.clone(),
                }
                .validate()?;
            }
            if !seen.insert(&input.source)
                || !matches!(
                    input.role,
                    DerivationInputRole::Source | DerivationInputRole::SampledFrame
                )
            {
                return Err(Error::invalid(
                    "prepared media inputs must be distinct physical source deliveries",
                ));
            }
            if let Some(at_ms) = input.at_ms
                && duration_ms.is_none_or(|duration| at_ms > duration)
            {
                return Err(Error::invalid(
                    "prepared frame time is outside selected media duration",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDerivationRun {
    pub job: JobRecord,
    pub request: DeriveMediaRequest,
    pub source_event_kind: ObservationKind,
    pub description_target: Option<RepresentationRef>,
    pub result_target: RepresentationRef,
    pub prepared: Option<MediaPreparedGraph>,
    pub description: Option<RepresentationRef>,
    pub result: Option<RepresentationRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRegionRequest {
    pub subject_id: SubjectId,
    pub material_id: MaterialId,
    pub parent: Option<SourceRegionId>,
    pub coordinate: SourceCoordinate,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivedRegionRequest {
    pub subject_id: SubjectId,
    pub representation: RepresentationRef,
    pub parent: Option<DerivedRegionId>,
    pub selector: DerivedSelector,
}
