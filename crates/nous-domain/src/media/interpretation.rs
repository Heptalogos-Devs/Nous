//! Typed material interpretation, evidence channels and coverage.
// Source channels, coverage/basis distinctions, exact support and local bounds.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceChannel {
    Visual,
    Audio,
    SourceText,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    NotAvailable,
    Observed,
    Reported,
    Limited,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterpretationCoverage {
    pub visual: CoverageStatus,
    pub audio: CoverageStatus,
    pub embedded_text: CoverageStatus,
    pub source_text: CoverageStatus,
}
impl InterpretationCoverage {
    fn for_channel(self, channel: EvidenceChannel) -> CoverageStatus {
        match channel {
            EvidenceChannel::Visual => self.visual,
            EvidenceChannel::Audio => self.audio,
            EvidenceChannel::SourceText => self.source_text,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterpretationBasis {
    Direct,
    Reported,
    Inferred,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaObservationKind {
    Object,
    Action,
    State,
    Scene,
    SpatialRelation,
    Sound,
    Music,
    Text,
    Speech,
    Event,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterpretationCertainty {
    Clear,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MentionCategory {
    Person,
    Organization,
    Place,
    Object,
    Concept,
    Work,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextFidelity {
    Verbatim,
    Approximate,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpeechFidelity {
    Verbatim,
    Semantic,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterpretationStatus {
    Supported,
    Tentative,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaSummary {
    pub content: String,
    pub basis_refs: Vec<MediaReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaObservation {
    pub kind: MediaObservationKind,
    pub content: String,
    pub evidence_channel: EvidenceChannel,
    pub basis: InterpretationBasis,
    pub certainty: InterpretationCertainty,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub basis_refs: Vec<MediaReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaMention {
    pub surface: String,
    pub category: MentionCategory,
    pub role: Option<String>,
    pub basis_refs: Vec<MediaReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaTextSpan {
    pub text: String,
    pub fidelity: TextFidelity,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub basis_refs: Vec<MediaReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaSpeechSpan {
    pub text: String,
    pub fidelity: SpeechFidelity,
    pub speaker_hint: Option<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub basis_refs: Vec<MediaReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaClaim {
    pub content: String,
    pub status: InterpretationStatus,
    pub basis_refs: Vec<MediaReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaUncertainty {
    pub issue: String,
    pub alternatives: Vec<String>,
    pub basis_refs: Vec<MediaReference>,
}
/// Material interpretation describes supplied media; it does not adopt Memory,
/// Concept, Episode or any other persistent cognition proposal.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaInterpretation {
    pub summary: MediaSummary,
    pub coverage: InterpretationCoverage,
    pub observations: Vec<MediaObservation>,
    pub mentions: Vec<MediaMention>,
    pub embedded_text: Vec<MediaTextSpan>,
    pub source_text: Vec<MediaTextSpan>,
    pub speech: Vec<MediaSpeechSpan>,
    pub interpretations: Vec<MediaClaim>,
    pub uncertainties: Vec<MediaUncertainty>,
}
impl MediaInterpretation {
    pub fn source_references(&self) -> Vec<MediaReference> {
        self.summary
            .basis_refs
            .iter()
            .chain(self.observations.iter().flat_map(|item| &item.basis_refs))
            .chain(self.mentions.iter().flat_map(|item| &item.basis_refs))
            .chain(self.embedded_text.iter().flat_map(|item| &item.basis_refs))
            .chain(self.source_text.iter().flat_map(|item| &item.basis_refs))
            .chain(self.speech.iter().flat_map(|item| &item.basis_refs))
            .chain(
                self.interpretations
                    .iter()
                    .flat_map(|item| &item.basis_refs),
            )
            .chain(self.uncertainties.iter().flat_map(|item| &item.basis_refs))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn validate(
        &self,
        access: ChannelAccess,
        catalog: &BTreeSet<MediaReference>,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        if serde_json::to_vec(self)?.len() > 262_144
            || [
                self.observations.len(),
                self.mentions.len(),
                self.embedded_text.len(),
                self.source_text.len(),
                self.speech.len(),
                self.interpretations.len(),
                self.uncertainties.len(),
            ]
            .iter()
            .any(|len| *len > 64)
        {
            return Err(Error::invalid(
                "media interpretation exceeds the saved source contract bounds",
            ));
        }
        bounded_text(&self.summary.content, 8192, "media summary", false)?;
        validate_basis(
            &self.summary.basis_refs,
            catalog,
            !self.summary.content.trim().is_empty(),
        )?;
        for (status, mode) in [
            (self.coverage.visual, access.visual),
            (self.coverage.audio, access.audio),
            (self.coverage.embedded_text, access.visual),
            (self.coverage.source_text, access.source_text),
        ] {
            if (mode == ChannelMode::Unavailable && status != CoverageStatus::NotAvailable)
                || (status == CoverageStatus::Observed && mode != ChannelMode::Original)
            {
                return Err(Error::invalid(
                    "media coverage invents an unavailable or directly observed source channel",
                ));
            }
        }
        for item in &self.observations {
            bounded_text(&item.content, 4096, "media observation", true)?;
            validate_basis(
                &item.basis_refs,
                catalog,
                item.basis != InterpretationBasis::Inferred,
            )?;
            validate_time(item.start_ms, item.end_ms, duration_ms)?;
            let mode = access.mode(item.evidence_channel);
            if mode == ChannelMode::Unavailable
                || self.coverage.for_channel(item.evidence_channel) == CoverageStatus::NotAvailable
                || (item.basis == InterpretationBasis::Direct && mode != ChannelMode::Original)
            {
                return Err(Error::invalid(
                    "observation basis upgrades represented or unavailable input into direct observation",
                ));
            }
        }
        for item in &self.mentions {
            bounded_text(&item.surface, 4096, "mention surface", true)?;
            if let Some(role) = &item.role {
                bounded_text(role, 4096, "mention role", false)?;
            }
            validate_basis(&item.basis_refs, catalog, false)?;
        }
        if (!self.embedded_text.is_empty() && access.visual == ChannelMode::Unavailable)
            || (!self.source_text.is_empty() && access.source_text == ChannelMode::Unavailable)
            || (!self.speech.is_empty() && access.audio == ChannelMode::Unavailable)
        {
            return Err(Error::invalid(
                "linguistic output invents an unavailable source channel",
            ));
        }
        for item in self.embedded_text.iter().chain(&self.source_text) {
            bounded_text(&item.text, 4096, "media text span", true)?;
            validate_time(item.start_ms, item.end_ms, duration_ms)?;
            validate_basis(&item.basis_refs, catalog, false)?;
        }
        for item in &self.speech {
            bounded_text(&item.text, 4096, "speech span", true)?;
            if let Some(speaker) = &item.speaker_hint {
                bounded_text(speaker, 4096, "speaker hint", false)?;
            }
            validate_time(item.start_ms, item.end_ms, duration_ms)?;
            validate_basis(&item.basis_refs, catalog, false)?;
        }
        for item in &self.interpretations {
            bounded_text(&item.content, 4096, "media interpretation", true)?;
            validate_basis(
                &item.basis_refs,
                catalog,
                item.status == InterpretationStatus::Supported,
            )?;
        }
        for item in &self.uncertainties {
            bounded_text(&item.issue, 4096, "media uncertainty", true)?;
            validate_basis(&item.basis_refs, catalog, false)?;
            if item.alternatives.len() > 8 {
                return Err(Error::invalid(
                    "media uncertainty exceeds the alternatives bound",
                ));
            }
            for alternative in &item.alternatives {
                bounded_text(alternative, 4096, "media alternative", true)?;
            }
        }
        Ok(())
    }
}
impl InterpretationSelector {
    pub(super) fn exists(&self, value: &MediaInterpretation) -> bool {
        match self {
            Self::Summary => true,
            Self::Observation { index } => (*index as usize) < value.observations.len(),
            Self::Mention { index } => (*index as usize) < value.mentions.len(),
            Self::EmbeddedText { index } => (*index as usize) < value.embedded_text.len(),
            Self::SourceText { index } => (*index as usize) < value.source_text.len(),
            Self::Speech { index } => (*index as usize) < value.speech.len(),
            Self::Interpretation { index } => (*index as usize) < value.interpretations.len(),
            Self::Uncertainty { index } => (*index as usize) < value.uncertainties.len(),
        }
    }
}
fn validate_basis(
    refs: &[MediaReference],
    catalog: &BTreeSet<MediaReference>,
    required: bool,
) -> Result<()> {
    if refs.len() > 16 || (required && refs.is_empty()) {
        return Err(Error::invalid(
            "media item needs bounded exact source support",
        ));
    }
    let mut seen = BTreeSet::new();
    for reference in refs {
        reference.validate()?;
        if !catalog.contains(reference) || !seen.insert(reference) {
            return Err(Error::invalid(
                "media item refers outside the admitted support catalog",
            ));
        }
    }
    Ok(())
}
fn validate_time(start: Option<u64>, end: Option<u64>, duration: Option<u64>) -> Result<()> {
    if matches!((start,end),(Some(a),Some(b))if a>b)
        || start
            .into_iter()
            .chain(end)
            .any(|at| duration.is_some_and(|duration| at > duration))
    {
        return Err(Error::invalid(
            "media timestamp is outside supplied source duration",
        ));
    }
    Ok(())
}
