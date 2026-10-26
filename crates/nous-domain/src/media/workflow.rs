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
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaPreparedGraph {
    pub inputs: Vec<MediaPreparedInput>,
    pub quality: DerivationQuality,
    pub preprocessing_digest: String,
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
