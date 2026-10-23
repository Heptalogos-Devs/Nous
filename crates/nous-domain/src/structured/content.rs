//! Typed cognitive content, epistemic stance and semantic roles.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see UPSTREAM.md.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveRole {
    Experiential,
    Declarative,
    ProceduralExperience,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormationMode {
    Grounded,
    Synthesized,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicClass {
    Observed,
    Reported,
    Derived,
    Inferred,
    Narrative,
    Simulated,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EpistemicStance {
    pub class: EpistemicClass,
    pub attribution: Option<String>,
    pub confidence: Option<f64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineageRelation {
    DerivedFrom,
    MergedFrom,
    SplitFrom,
    ResegmentedFrom,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineageLink {
    pub relation: LineageRelation,
    pub target: CognitiveRef,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemorySemantics {
    pub memory_id: MemoryId,
    pub memory_revision: i64,
    pub cognitive_role: CognitiveRole,
    pub formation_mode: FormationMode,
    pub epistemic: EpistemicStance,
    pub grounding_observation: Option<ObservationId>,
    pub concepts: Vec<CognitiveRef>,
    pub qualifications: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConceptContent {
    pub label: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub canonical: Option<CognitiveRef>,
    pub members: Vec<CognitiveRef>,
    pub lineage: Vec<LineageLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EpisodeMember {
    pub source: EvidenceSource,
    pub role: String,
    pub ordinal: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EpisodeContent {
    pub title: String,
    pub track: String,
    pub experience: TimeRange,
    pub parent: Option<CognitiveRef>,
    pub members: Vec<EpisodeMember>,
    pub boundary_explanation: String,
    pub lineage: Vec<LineageLink>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JournalPointRole {
    Summary,
    Outcome,
    Change,
    Decision,
    OpenQuestion,
    SalientEvent,
    Reflection,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalPoint {
    pub role: JournalPointRole,
    pub text: String,
    pub evidence: Vec<EvidenceLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalContent {
    pub title: String,
    pub narrative: String,
    pub period: TimeRange,
    pub knowledge_cutoff: i64,
    pub perspective: EpistemicStance,
    pub points: Vec<JournalPoint>,
    pub episodes: Vec<CognitiveRef>,
    pub lineage: Vec<LineageLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaScope {
    pub description: String,
    pub about: Vec<CognitiveRef>,
    pub concepts: Vec<CognitiveRef>,
    pub validity: Option<TimeRange>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaContent {
    pub title: String,
    pub claim: String,
    pub scope: SchemaScope,
    pub boundary: String,
    pub formation_mode: FormationMode,
    pub epistemic: EpistemicStance,
    pub support: Vec<EvidenceSource>,
    pub counterexamples: Vec<EvidenceSource>,
    pub boundary_cases: Vec<EvidenceSource>,
    pub lineage: Vec<LineageLink>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssociationPolarity {
    Positive,
    Negative,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssociationBasisClass {
    HostExplicit,
    SourceEvidence,
    CognitiveDerivation,
    MeaningfulUse,
    DerivedStructure,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssociationContent {
    pub from: CognitiveRef,
    pub to: CognitiveRef,
    pub relation: String,
    pub polarity: AssociationPolarity,
    pub basis_class: AssociationBasisClass,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "content",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CognitiveContent {
    MemorySemantics(MemorySemantics),
    Concept(ConceptContent),
    Episode(EpisodeContent),
    Journal(JournalContent),
    Schema(SchemaContent),
    Association(AssociationContent),
}

impl EpistemicStance {
    pub fn validate(&self) -> Result<()> {
        if self
            .confidence
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(Error::invalid(
                "epistemic confidence must be finite and within 0..1",
            ));
        }
        if let Some(attribution) = &self.attribution {
            text(attribution, "epistemic attribution", 65536)?;
        }
        Ok(())
    }
}

impl CognitiveContent {
    pub fn kind(&self) -> CognitiveKind {
        match self {
            Self::MemorySemantics(_) => CognitiveKind::MemorySemantics,
            Self::Concept(_) => CognitiveKind::Concept,
            Self::Episode(_) => CognitiveKind::Episode,
            Self::Journal(_) => CognitiveKind::Journal,
            Self::Schema(_) => CognitiveKind::Schema,
            Self::Association(_) => CognitiveKind::Association,
        }
    }
}

impl ConceptContent {
    pub fn is_active(&self) -> bool {
        self.canonical.is_none()
    }
    pub fn alias_keys(&self) -> Vec<String> {
        std::iter::once(&self.label)
            .chain(&self.aliases)
            .map(|label| label.trim().to_lowercase())
            .collect()
    }
}
