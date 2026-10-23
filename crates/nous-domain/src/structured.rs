//! Typed cognitive objects adapted from previous Nous memory/core domains.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Sources: Nous Wave 412b062082afb75ccfae4c3f69fc606d3e3cc862.
// See UPSTREAM.md for exact source paths and adaptation boundaries.
use crate::{
    CognitiveId, CognitiveTimes, Error, MaterialId, MemoryId, ModelCallId, NamespaceId,
    ObservationId, Result, SubjectId, TimeRange,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveKind {
    Memory,
    MemorySemantics,
    Concept,
    Episode,
    Journal,
    Schema,
    Association,
}
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct CognitiveRef {
    pub kind: CognitiveKind,
    pub id: String,
    pub revision: i64,
}
impl CognitiveRef {
    pub fn memory(id: &MemoryId, revision: i64) -> Self {
        Self {
            kind: CognitiveKind::Memory,
            id: id.to_string(),
            revision,
        }
    }
    pub fn structured(kind: CognitiveKind, id: &CognitiveId, revision: i64) -> Self {
        Self {
            kind,
            id: id.to_string(),
            revision,
        }
    }
    pub fn validate(&self) -> Result<()> {
        let id = uuid::Uuid::parse_str(&self.id)
            .map_err(|_| Error::invalid("cognitive reference ID must be a UUID"))?;
        if id.is_nil() || id.to_string() != self.id || self.revision <= 0 {
            return Err(Error::invalid(
                "cognitive reference needs canonical UUID and exact positive revision",
            ));
        }
        Ok(())
    }
}
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceSource {
    Material { material_id: MaterialId },
    Observation { observation_id: ObservationId },
    MemoryRevision { memory_id: MemoryId, revision: i64 },
    CognitiveRevision { reference: CognitiveRef },
}
impl EvidenceSource {
    pub fn cognitive_ref(&self) -> Option<CognitiveRef> {
        match self {
            Self::MemoryRevision {
                memory_id,
                revision,
            } => Some(CognitiveRef::memory(memory_id, *revision)),
            Self::CognitiveRevision { reference } => Some(reference.clone()),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Material { material_id } => validate_uuid(material_id.as_str()),
            Self::Observation { observation_id } => validate_uuid(observation_id.as_str()),
            Self::MemoryRevision {
                memory_id,
                revision,
            } => CognitiveRef::memory(memory_id, *revision).validate(),
            Self::CognitiveRevision { reference } => {
                reference.validate()?;
                if reference.kind == CognitiveKind::Memory {
                    return Err(Error::invalid(
                        "Memory evidence uses memory_revision source",
                    ));
                }
                Ok(())
            }
        }
    }
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePolarity {
    Support,
    Contradict,
    Context,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicRelation {
    Supports,
    Contradicts,
    Corroborates,
    Weakens,
    Corrects,
    Counterexample,
    InferredFrom,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceLink {
    pub source: EvidenceSource,
    pub polarity: EvidencePolarity,
    pub relation: Option<EpistemicRelation>,
    pub explanation: String,
}
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredObject {
    pub id: CognitiveId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
    pub times: CognitiveTimes,
    pub producer_call_id: Option<ModelCallId>,
    pub recorded_at: String,
    pub commit_seq: i64,
    pub deleted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateStructuredRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub expected_revision: i64,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
    pub times: CognitiveTimes,
    pub producer_call_id: Option<ModelCallId>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviseStructuredRequest {
    pub id: CognitiveId,
    pub expected_revision: i64,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
    pub times: CognitiveTimes,
    pub producer_call_id: Option<ModelCallId>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredTargetSlot {
    pub key: String,
    pub kind: CognitiveKind,
    pub id: CognitiveId,
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredProposalChange {
    pub slot_key: String,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredProposalGroup {
    pub key: String,
    pub depends_on: Vec<String>,
    pub changes: Vec<StructuredProposalChange>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredProposal {
    pub groups: Vec<StructuredProposalGroup>,
}

impl EvidenceLink {
    pub fn validate(&self) -> Result<()> {
        self.source.validate()?;
        optional_text(&self.explanation, "evidence explanation", 65536)?;
        let compatible = match self.relation {
            Some(
                EpistemicRelation::Supports
                | EpistemicRelation::Corroborates
                | EpistemicRelation::InferredFrom,
            ) => self.polarity == EvidencePolarity::Support,
            Some(
                EpistemicRelation::Contradicts
                | EpistemicRelation::Weakens
                | EpistemicRelation::Counterexample,
            ) => self.polarity == EvidencePolarity::Contradict,
            Some(EpistemicRelation::Corrects) => self.polarity != EvidencePolarity::Context,
            None => true,
        };
        if !compatible {
            return Err(Error::invalid(
                "epistemic relation disagrees with evidence polarity",
            ));
        }
        Ok(())
    }
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
    /// All semantic and support references; their visibility and exact versions are
    /// resolved by storage. Context references do not become positive evidence.
    pub fn references(&self) -> Vec<CognitiveRef> {
        self.evidence_links()
            .iter()
            .filter_map(|link| link.source.cognitive_ref())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn sources(&self) -> Vec<EvidenceSource> {
        self.evidence_links()
            .into_iter()
            .map(|link| link.source)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn evidence_links(&self) -> Vec<EvidenceLink> {
        let mut links = Vec::new();
        let mut add = |source, polarity, explanation: String| {
            links.push(EvidenceLink {
                source,
                polarity,
                relation: None,
                explanation,
            })
        };
        match self {
            Self::MemorySemantics(value) => {
                add(
                    EvidenceSource::MemoryRevision {
                        memory_id: value.memory_id.clone(),
                        revision: value.memory_revision,
                    },
                    EvidencePolarity::Support,
                    "annotated Memory revision".into(),
                );
                if let Some(id) = &value.grounding_observation {
                    add(
                        EvidenceSource::Observation {
                            observation_id: id.clone(),
                        },
                        EvidencePolarity::Context,
                        "grounding anchor; explicit support is recorded separately".into(),
                    );
                }
                for reference in &value.concepts {
                    add(
                        source_ref(reference),
                        EvidencePolarity::Context,
                        "concept annotation".into(),
                    );
                }
            }
            Self::Concept(value) => {
                if let Some(reference) = &value.canonical {
                    add(
                        source_ref(reference),
                        EvidencePolarity::Context,
                        "canonical concept".into(),
                    );
                }
                for reference in &value.members {
                    add(
                        source_ref(reference),
                        EvidencePolarity::Context,
                        "concept member".into(),
                    );
                }
                for lineage in &value.lineage {
                    add(
                        source_ref(&lineage.target),
                        EvidencePolarity::Context,
                        lineage.reason.clone(),
                    );
                }
            }
            Self::Episode(value) => {
                if let Some(reference) = &value.parent {
                    add(
                        source_ref(reference),
                        EvidencePolarity::Context,
                        "parent episode".into(),
                    );
                }
                for member in &value.members {
                    add(
                        member.source.clone(),
                        if member.role == "context" {
                            EvidencePolarity::Context
                        } else {
                            EvidencePolarity::Support
                        },
                        member.role.clone(),
                    );
                }
                for lineage in &value.lineage {
                    add(
                        source_ref(&lineage.target),
                        EvidencePolarity::Context,
                        lineage.reason.clone(),
                    );
                }
            }
            Self::Journal(value) => {
                for episode in &value.episodes {
                    add(
                        source_ref(episode),
                        EvidencePolarity::Support,
                        "journal source episode".into(),
                    );
                }
                for lineage in &value.lineage {
                    add(
                        source_ref(&lineage.target),
                        EvidencePolarity::Context,
                        lineage.reason.clone(),
                    );
                }
                for point in &value.points {
                    links.extend(point.evidence.iter().cloned());
                }
            }
            Self::Schema(value) => {
                for source in &value.support {
                    add(
                        source.clone(),
                        EvidencePolarity::Support,
                        "schema support".into(),
                    );
                }
                for source in &value.counterexamples {
                    add(
                        source.clone(),
                        EvidencePolarity::Contradict,
                        "schema counterexample".into(),
                    );
                }
                for source in &value.boundary_cases {
                    add(
                        source.clone(),
                        EvidencePolarity::Context,
                        "schema boundary case".into(),
                    );
                }
                for reference in value.scope.about.iter().chain(&value.scope.concepts) {
                    add(
                        source_ref(reference),
                        EvidencePolarity::Context,
                        "applicability scope".into(),
                    );
                }
                for lineage in &value.lineage {
                    add(
                        source_ref(&lineage.target),
                        EvidencePolarity::Context,
                        lineage.reason.clone(),
                    );
                }
            }
            Self::Association(value) => {
                add(
                    source_ref(&value.from),
                    EvidencePolarity::Context,
                    "association endpoint".into(),
                );
                add(
                    source_ref(&value.to),
                    EvidencePolarity::Context,
                    "association endpoint".into(),
                );
            }
        }
        dedup_links(links)
    }
    pub fn validate(&self, evidence: &[EvidenceLink]) -> Result<()> {
        validate_links(evidence)?;
        match self {
            Self::MemorySemantics(value) => {
                validate_uuid(value.memory_id.as_str())?;
                positive_revision(value.memory_revision)?;
                value.epistemic.validate()?;
                validate_refs(&value.concepts, Some(CognitiveKind::Concept))?;
                for item in &value.qualifications {
                    text(item, "memory qualification", 65536)?;
                }
                match value.formation_mode {
                    FormationMode::Grounded => {
                        let anchor = value.grounding_observation.as_ref().ok_or_else(|| {
                            Error::invalid(
                                "grounded Memory semantics require an observation anchor",
                            )
                        })?;
                        validate_uuid(anchor.as_str())?;
                        if !evidence.iter().any(|link| {
                            link.polarity == EvidencePolarity::Support
                                && link.source
                                    == EvidenceSource::Observation {
                                        observation_id: anchor.clone(),
                                    }
                        }) {
                            return Err(Error::invalid(
                                "grounding observation must be explicit support",
                            ));
                        }
                    }
                    FormationMode::Synthesized => {
                        if value.grounding_observation.is_some() {
                            return Err(Error::invalid(
                                "synthesized Memory semantics have no single grounding observation",
                            ));
                        }
                        require_synthesis_support(evidence)?;
                    }
                }
            }
            Self::Concept(value) => {
                short(&value.label, "concept label")?;
                optional_text(&value.description, "concept description", 1_000_000)?;
                let mut aliases = BTreeSet::from([value.label.trim().to_lowercase()]);
                for alias in &value.aliases {
                    short(alias, "concept alias")?;
                    if !aliases.insert(alias.trim().to_lowercase()) {
                        return Err(Error::invalid("duplicate canonical concept label or alias"));
                    }
                }
                if let Some(canonical) = &value.canonical {
                    validate_ref_kind(canonical, CognitiveKind::Concept)?;
                }
                validate_refs(&value.members, None)?;
                validate_lineage(&value.lineage, CognitiveKind::Concept)?;
            }
            Self::Episode(value) => {
                text(&value.title, "episode title", 1024)?;
                short(&value.track, "episode track")?;
                validate_range(&value.experience)?;
                text(&value.boundary_explanation, "episode boundary", 65536)?;
                if let Some(parent) = &value.parent {
                    validate_ref_kind(parent, CognitiveKind::Episode)?;
                }
                if value.members.is_empty() {
                    return Err(Error::invalid("episode requires ordered source members"));
                }
                let mut seen = BTreeSet::new();
                for (ordinal, member) in value.members.iter().enumerate() {
                    member.source.validate()?;
                    short(&member.role, "episode member role")?;
                    if member.ordinal as usize != ordinal || !seen.insert(member.source.clone()) {
                        return Err(Error::invalid(
                            "episode members require unique sources and contiguous zero-based ordinals",
                        ));
                    }
                }
                validate_lineage(&value.lineage, CognitiveKind::Episode)?;
            }
            Self::Journal(value) => {
                text(&value.title, "journal title", 1024)?;
                text(&value.narrative, "journal narrative", 1_000_000)?;
                validate_range(&value.period)?;
                value.perspective.validate()?;
                if value.knowledge_cutoff < 0
                    || value.episodes.is_empty()
                    || value.points.is_empty()
                {
                    return Err(Error::invalid(
                        "journal requires a knowledge cutoff, source episodes and supported points",
                    ));
                }
                validate_refs(&value.episodes, Some(CognitiveKind::Episode))?;
                for point in &value.points {
                    text(&point.text, "journal point", 65536)?;
                    if point.evidence.is_empty() {
                        return Err(Error::invalid(
                            "every journal point needs explicit evidence",
                        ));
                    }
                    validate_links(&point.evidence)?;
                }
                validate_lineage(&value.lineage, CognitiveKind::Journal)?;
            }
            Self::Schema(value) => {
                text(&value.title, "schema title", 1024)?;
                text(&value.claim, "schema structural claim", 1_000_000)?;
                text(
                    &value.scope.description,
                    "schema applicability scope",
                    65536,
                )?;
                text(&value.boundary, "schema boundary", 65536)?;
                value.epistemic.validate()?;
                validate_refs(&value.scope.about, None)?;
                validate_refs(&value.scope.concepts, Some(CognitiveKind::Concept))?;
                if let Some(validity) = &value.scope.validity {
                    validate_range(validity)?;
                }
                if value.support.is_empty() {
                    return Err(Error::invalid(
                        "schema needs support; counterexamples and boundaries have independent roles",
                    ));
                }
                validate_sources(&value.support)?;
                validate_sources(&value.counterexamples)?;
                validate_sources(&value.boundary_cases)?;
                if value.formation_mode == FormationMode::Synthesized && value.support.len() < 2 {
                    return Err(Error::invalid(
                        "synthesized schema needs at least two distinct source inputs",
                    ));
                }
                validate_lineage(&value.lineage, CognitiveKind::Schema)?;
            }
            Self::Association(value) => {
                value.from.validate()?;
                value.to.validate()?;
                short(&value.relation, "association relation")?;
                optional_text(&value.description, "association description", 65536)?;
                if value.from.kind == value.to.kind && value.from.id == value.to.id {
                    return Err(Error::invalid(
                        "association endpoints must be different cognitive objects",
                    ));
                }
            }
        }
        let all = self
            .evidence_links()
            .into_iter()
            .chain(evidence.iter().cloned())
            .collect::<Vec<_>>();
        for link in &all {
            link.validate()?;
        }
        if !all
            .iter()
            .any(|link| link.polarity == EvidencePolarity::Support)
        {
            return Err(Error::invalid(
                "cognitive content needs positive source support",
            ));
        }
        Ok(())
    }
}
impl ConceptContent {
    pub fn alias_keys(&self) -> Vec<String> {
        std::iter::once(&self.label)
            .chain(&self.aliases)
            .map(|label| label.trim().to_lowercase())
            .collect()
    }
}
impl StructuredObject {
    pub fn kind(&self) -> CognitiveKind {
        self.body.kind()
    }
    pub fn reference(&self) -> CognitiveRef {
        CognitiveRef::structured(self.kind(), &self.id, self.revision)
    }
    pub fn evidence_links(&self) -> Vec<EvidenceLink> {
        dedup_links(
            self.evidence
                .iter()
                .cloned()
                .chain(self.body.evidence_links())
                .collect(),
        )
    }
    pub fn sources(&self) -> Vec<EvidenceSource> {
        self.evidence_links()
            .into_iter()
            .map(|link| link.source)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn references(&self) -> Vec<CognitiveRef> {
        self.sources()
            .iter()
            .filter_map(EvidenceSource::cognitive_ref)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn validate(&self) -> Result<()> {
        validate_uuid(self.id.as_str())?;
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.subject_id.as_str())?;
        positive_revision(self.revision)?;
        self.body.validate(&self.evidence)?;
        validate_times(&self.times)?;
        text(&self.recorded_at, "recorded_at", 128)?;
        if self.commit_seq < 0 {
            return Err(Error::invalid("commit_seq must be nonnegative"));
        }
        if let Some(id) = &self.producer_call_id {
            validate_uuid(id.as_str())?;
        }
        reject_self_reference(&self.body, &self.evidence, &self.id)?;
        Ok(())
    }
}
impl CreateStructuredRequest {
    pub fn validate(&self) -> Result<()> {
        if self.expected_revision != 0 {
            return Err(Error::invalid("create expected_revision must be zero"));
        }
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.subject_id.as_str())?;
        self.body.validate(&self.evidence)?;
        validate_times(&self.times)?;
        if let Some(id) = &self.producer_call_id {
            validate_uuid(id.as_str())?;
        }
        Ok(())
    }
}
impl ReviseStructuredRequest {
    pub fn validate(&self) -> Result<()> {
        positive_revision(self.expected_revision)?;
        validate_uuid(self.id.as_str())?;
        self.body.validate(&self.evidence)?;
        validate_times(&self.times)?;
        reject_self_reference(&self.body, &self.evidence, &self.id)?;
        if let Some(id) = &self.producer_call_id {
            validate_uuid(id.as_str())?;
        }
        Ok(())
    }
}
impl StructuredTargetSlot {
    pub fn output_reference(&self) -> Result<CognitiveRef> {
        self.validate()?;
        let revision = self
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::invalid("target revision overflow"))?;
        Ok(CognitiveRef::structured(self.kind, &self.id, revision))
    }
    pub fn validate(&self) -> Result<()> {
        short(&self.key, "target slot key")?;
        validate_uuid(self.id.as_str())?;
        if self.kind == CognitiveKind::Memory || self.expected_revision < 0 {
            return Err(Error::invalid(
                "structured slots need a structured kind and nonnegative expected revision",
            ));
        }
        Ok(())
    }
}
impl StructuredProposalChange {
    pub fn validate(&self) -> Result<()> {
        short(&self.slot_key, "proposal slot key")?;
        self.body.validate(&self.evidence)
    }
    pub fn evidence_links(&self) -> Vec<EvidenceLink> {
        dedup_links(
            self.evidence
                .iter()
                .cloned()
                .chain(self.body.evidence_links())
                .collect(),
        )
    }
    pub fn sources(&self) -> Vec<EvidenceSource> {
        self.evidence_links()
            .into_iter()
            .map(|link| link.source)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}
impl StructuredProposalGroup {
    pub fn validate(&self) -> Result<()> {
        short(&self.key, "proposal group key")?;
        if self.changes.is_empty() {
            return Err(Error::invalid("proposal group must contain changes"));
        }
        let mut seen = BTreeSet::new();
        for change in &self.changes {
            change.validate()?;
            if !seen.insert(&change.slot_key) {
                return Err(Error::invalid("duplicate target slot within one group"));
            }
        }
        let mut dependencies = BTreeSet::new();
        for dependency in &self.depends_on {
            short(dependency, "group dependency")?;
            if dependency == &self.key || !dependencies.insert(dependency) {
                return Err(Error::invalid("invalid or duplicate group dependency"));
            }
        }
        Ok(())
    }
}
impl StructuredProposal {
    /// Only proposal topology is global. A caller can validate/compile individual
    /// groups so a rejected group does not discard independently valid groups.
    pub fn validate_structure(&self) -> Result<()> {
        if self.groups.is_empty() {
            return Err(Error::invalid("structured proposal needs groups"));
        }
        let mut previous = BTreeSet::new();
        let mut slots = BTreeSet::new();
        for group in &self.groups {
            short(&group.key, "proposal group key")?;
            if group.depends_on.iter().any(|key| !previous.contains(key))
                || !previous.insert(group.key.clone())
            {
                return Err(Error::invalid(
                    "groups must have unique keys and topologically earlier dependencies",
                ));
            }
            for change in &group.changes {
                if !slots.insert(change.slot_key.clone()) {
                    return Err(Error::invalid(
                        "one proposal may write each target slot once",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_structure()?;
        for group in &self.groups {
            group.validate()?;
        }
        Ok(())
    }
    pub fn validate_against(
        &self,
        slots: &[StructuredTargetSlot],
        catalog: &[EvidenceSource],
    ) -> Result<()> {
        self.validate()?;
        let mut bound = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for slot in slots {
            slot.validate()?;
            if bound.insert(slot.key.clone(), slot).is_some() || !ids.insert(slot.id.clone()) {
                return Err(Error::invalid(
                    "target slots require unique keys and object IDs",
                ));
            }
        }
        let mut sources = BTreeSet::new();
        for source in catalog {
            source.validate()?;
            sources.insert(source.clone());
        }
        let owners = self
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .changes
                    .iter()
                    .map(move |change| (&change.slot_key, &group.key))
            })
            .collect::<BTreeMap<_, _>>();
        let mut ancestors = BTreeMap::<String, BTreeSet<String>>::new();
        for group in &self.groups {
            let mut allowed_groups = BTreeSet::from([group.key.clone()]);
            for dependency in &group.depends_on {
                allowed_groups.insert(dependency.clone());
                if let Some(prior) = ancestors.get(dependency) {
                    allowed_groups.extend(prior.iter().cloned());
                }
            }
            for change in &group.changes {
                let slot = bound
                    .get(&change.slot_key)
                    .ok_or_else(|| Error::invalid("proposal targets an undeclared slot"))?;
                if slot.kind != change.body.kind() {
                    return Err(Error::invalid(
                        "proposal content kind differs from target slot",
                    ));
                }
                reject_self_reference(&change.body, &change.evidence, &slot.id)?;
                for source in change.sources() {
                    if sources.contains(&source) {
                        continue;
                    }
                    let target = source.cognitive_ref().and_then(|reference| {
                        bound
                            .values()
                            .find(|target| {
                                target.output_reference().ok().as_ref() == Some(&reference)
                            })
                            .copied()
                    });
                    let permitted = target.is_some_and(|target| {
                        owners
                            .get(&target.key)
                            .is_some_and(|owner| allowed_groups.contains(*owner))
                    });
                    if !permitted {
                        return Err(Error::invalid(
                            "proposal source lies outside catalog or declared output dependencies",
                        ));
                    }
                }
            }
            ancestors.insert(group.key.clone(), allowed_groups);
        }
        Ok(())
    }
}
fn source_ref(reference: &CognitiveRef) -> EvidenceSource {
    if reference.kind == CognitiveKind::Memory {
        EvidenceSource::MemoryRevision {
            memory_id: MemoryId(reference.id.clone()),
            revision: reference.revision,
        }
    } else {
        EvidenceSource::CognitiveRevision {
            reference: reference.clone(),
        }
    }
}
fn dedup_links(links: Vec<EvidenceLink>) -> Vec<EvidenceLink> {
    let mut seen = BTreeSet::new();
    links
        .into_iter()
        .filter(|link| seen.insert((link.source.clone(), link.polarity, link.relation)))
        .collect()
}
fn validate_links(links: &[EvidenceLink]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for link in links {
        link.validate()?;
        if !seen.insert((link.source.clone(), link.polarity, link.relation)) {
            return Err(Error::invalid("duplicate evidence link"));
        }
    }
    Ok(())
}
fn validate_sources(sources: &[EvidenceSource]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for source in sources {
        source.validate()?;
        if !seen.insert(source) {
            return Err(Error::invalid("duplicate source within evidence role"));
        }
    }
    Ok(())
}
fn validate_refs(refs: &[CognitiveRef], kind: Option<CognitiveKind>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for reference in refs {
        reference.validate()?;
        if kind.is_some_and(|kind| reference.kind != kind) || !seen.insert(reference) {
            return Err(Error::invalid("reference kind or uniqueness is invalid"));
        }
    }
    Ok(())
}
fn validate_ref_kind(reference: &CognitiveRef, kind: CognitiveKind) -> Result<()> {
    validate_refs(std::slice::from_ref(reference), Some(kind))
}
fn validate_lineage(links: &[LineageLink], kind: CognitiveKind) -> Result<()> {
    let mut seen = BTreeSet::new();
    for link in links {
        link.target.validate()?;
        text(&link.reason, "lineage reason", 65536)?;
        if !seen.insert((link.target.clone(), link.relation as u8)) {
            return Err(Error::invalid("duplicate lineage link"));
        }
        let valid = match link.relation {
            LineageRelation::DerivedFrom => true,
            LineageRelation::MergedFrom | LineageRelation::SplitFrom => {
                matches!(kind, CognitiveKind::Concept | CognitiveKind::Schema)
                    && link.target.kind == kind
            }
            LineageRelation::ResegmentedFrom => {
                kind == CognitiveKind::Episode && link.target.kind == kind
            }
        };
        if !valid {
            return Err(Error::invalid(
                "lineage relation is incompatible with cognitive kinds",
            ));
        }
    }
    Ok(())
}
fn reject_self_reference(
    body: &CognitiveContent,
    evidence: &[EvidenceLink],
    id: &CognitiveId,
) -> Result<()> {
    let references = body.references().into_iter().chain(
        evidence
            .iter()
            .filter_map(|link| link.source.cognitive_ref()),
    );
    if references
        .into_iter()
        .any(|reference| reference.kind == body.kind() && reference.id == id.as_str())
    {
        return Err(Error::invalid(
            "a cognitive revision cannot depend on any revision of its own object",
        ));
    }
    Ok(())
}
fn require_synthesis_support(evidence: &[EvidenceLink]) -> Result<()> {
    let inputs = evidence
        .iter()
        .filter(|link| link.polarity == EvidencePolarity::Support)
        .map(|link| &link.source)
        .collect::<BTreeSet<_>>();
    if inputs.len() < 2 {
        return Err(Error::invalid(
            "synthesized cognition needs at least two distinct source inputs",
        ));
    }
    Ok(())
}
fn validate_times(times: &CognitiveTimes) -> Result<()> {
    times.formation.validate()?;
    times.recorded.validate()?;
    if let Some(time) = &times.event {
        time.validate()?;
    }
    if let Some(time) = &times.observation {
        time.validate()?;
    }
    if let Some(validity) = &times.validity {
        validate_range(validity)?;
    }
    Ok(())
}
fn validate_range(range: &TimeRange) -> Result<()> {
    short(&range.domain, "time domain")?;
    if matches!((range.start,range.end),(Some(a),Some(b))if a>b) {
        return Err(Error::invalid("time range starts after it ends"));
    }
    Ok(())
}
fn validate_uuid(id: &str) -> Result<()> {
    let parsed =
        uuid::Uuid::parse_str(id).map_err(|_| Error::invalid("identity must be a UUID"))?;
    if parsed.is_nil() || parsed.to_string() != id {
        return Err(Error::invalid("identity must be a canonical nonnil UUID"));
    }
    Ok(())
}
fn positive_revision(revision: i64) -> Result<()> {
    if revision <= 0 {
        return Err(Error::invalid("exact revision must be positive"));
    }
    Ok(())
}
fn short(value: &str, field: &str) -> Result<()> {
    crate::validate_name(value, field)
}
fn text(value: &str, field: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!("{field} is required")));
    }
    optional_text(value, field, max)
}
fn optional_text(value: &str, field: &str, max: usize) -> Result<()> {
    if value.len() > max || value.contains('\0') {
        return Err(Error::invalid(format!(
            "{field} exceeds its bound or contains NUL"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn support(source: EvidenceSource) -> EvidenceLink {
        EvidenceLink {
            source,
            polarity: EvidencePolarity::Support,
            relation: Some(EpistemicRelation::Supports),
            explanation: "actual source input".into(),
        }
    }
    fn material() -> EvidenceSource {
        EvidenceSource::Material {
            material_id: MaterialId::new(),
        }
    }
    fn stance() -> EpistemicStance {
        EpistemicStance {
            class: EpistemicClass::Reported,
            attribution: Some("source record".into()),
            confidence: Some(0.7),
        }
    }
    fn concept(label: &str) -> CognitiveContent {
        CognitiveContent::Concept(ConceptContent {
            label: label.into(),
            description: "supported concept meaning".into(),
            aliases: vec![],
            canonical: None,
            members: vec![],
            lineage: vec![],
        })
    }
    fn times() -> CognitiveTimes {
        CognitiveTimes {
            event: None,
            observation: None,
            formation: crate::TimePoint {
                domain: "utc".into(),
                coordinate: 1,
            },
            validity: None,
            recorded: crate::TimePoint {
                domain: "utc".into(),
                coordinate: 1,
            },
        }
    }
    #[test]
    fn formation_roles_stance_and_exact_support_are_separate() {
        let observation = ObservationId::new();
        let mut content = MemorySemantics {
            memory_id: MemoryId::new(),
            memory_revision: 1,
            cognitive_role: CognitiveRole::Declarative,
            formation_mode: FormationMode::Grounded,
            epistemic: stance(),
            grounding_observation: Some(observation.clone()),
            concepts: vec![],
            qualifications: vec!["reported rather than witnessed content".into()],
        };
        let source = support(EvidenceSource::Observation {
            observation_id: observation,
        });
        assert!(
            CognitiveContent::MemorySemantics(content.clone())
                .validate(std::slice::from_ref(&source))
                .is_ok()
        );
        assert!(
            CognitiveContent::MemorySemantics(content.clone())
                .validate(&[])
                .is_err()
        );
        content.formation_mode = FormationMode::Synthesized;
        content.grounding_observation = None;
        assert!(
            CognitiveContent::MemorySemantics(content.clone())
                .validate(std::slice::from_ref(&source))
                .is_err()
        );
        assert!(
            CognitiveContent::MemorySemantics(content)
                .validate(&[source, support(material())])
                .is_ok()
        );
        let malformed = serde_json::json!({"kind":"material","material_id":MaterialId::new(),"independent_roots":["claimed by caller"]});
        assert!(serde_json::from_value::<EvidenceSource>(malformed).is_err());
        assert!(
            CognitiveRef {
                kind: CognitiveKind::Concept,
                id: CognitiveId::new().to_string(),
                revision: 0
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn body_sources_preserve_counterevidence_context_and_all_exact_references() {
        let a = material();
        let b = material();
        let c = material();
        let concept_ref = CognitiveRef::structured(CognitiveKind::Concept, &CognitiveId::new(), 3);
        let body = CognitiveContent::Schema(SchemaContent {
            title: "group response pattern".into(),
            claim: "responds to narrow topics".into(),
            scope: SchemaScope {
                description: "large public groups".into(),
                about: vec![],
                concepts: vec![concept_ref.clone()],
                validity: None,
            },
            boundary: "private conversation is outside this pattern".into(),
            formation_mode: FormationMode::Grounded,
            epistemic: stance(),
            support: vec![a.clone()],
            counterexamples: vec![b.clone()],
            boundary_cases: vec![c.clone()],
            lineage: vec![],
        });
        body.validate(&[]).unwrap();
        let links = body.evidence_links();
        assert!(
            links
                .iter()
                .any(|link| link.source == a && link.polarity == EvidencePolarity::Support)
        );
        assert!(
            links
                .iter()
                .any(|link| link.source == b && link.polarity == EvidencePolarity::Contradict)
        );
        assert!(
            links
                .iter()
                .any(|link| link.source == c && link.polarity == EvidencePolarity::Context)
        );
        assert!(body.references().contains(&concept_ref));
        let mut object = StructuredObject {
            id: CognitiveId::new(),
            namespace_id: NamespaceId::new(),
            subject_id: SubjectId::new(),
            revision: 1,
            body,
            evidence: vec![],
            times: times(),
            producer_call_id: None,
            recorded_at: "2026-10-23T00:00:00Z".into(),
            commit_seq: 1,
            deleted: false,
        };
        object.validate().unwrap();
        assert_eq!(object.sources().len(), 4);
        object.body = concept("label");
        object.evidence = vec![support(material())];
        if let CognitiveContent::Concept(value) = &mut object.body {
            value.aliases = vec![" LABEL ".into()];
        }
        assert!(object.validate().is_err());
        if let CognitiveContent::Concept(value) = &mut object.body {
            value.aliases.clear();
            value.canonical = Some(CognitiveRef::structured(
                CognitiveKind::Concept,
                &object.id,
                1,
            ));
        }
        assert!(object.validate().is_err());
        let schema = schemars::schema_for!(StructuredObject);
        assert!(
            serde_json::to_value(schema)
                .unwrap()
                .to_string()
                .contains("MemorySemantics")
        );
    }
    #[test]
    fn episode_member_order_and_journal_supported_perspective_have_rules() {
        let source = material();
        let episode = CognitiveRef::structured(CognitiveKind::Episode, &CognitiveId::new(), 1);
        let mut content = EpisodeContent {
            title: "an actual work session".into(),
            track: "task".into(),
            experience: TimeRange {
                domain: "utc".into(),
                start: Some(1),
                end: Some(2),
            },
            parent: None,
            members: vec![EpisodeMember {
                source: source.clone(),
                role: "experience".into(),
                ordinal: 1,
            }],
            boundary_explanation: "session ended".into(),
            lineage: vec![],
        };
        assert!(
            CognitiveContent::Episode(content.clone())
                .validate(&[])
                .is_err()
        );
        content.members[0].ordinal = 0;
        CognitiveContent::Episode(content).validate(&[]).unwrap();
        let mut journal = JournalContent {
            title: "day narrative".into(),
            narrative: "what was understood then".into(),
            period: TimeRange {
                domain: "utc".into(),
                start: Some(1),
                end: Some(2),
            },
            knowledge_cutoff: 4,
            perspective: EpistemicStance {
                class: EpistemicClass::Narrative,
                attribution: None,
                confidence: None,
            },
            points: vec![JournalPoint {
                role: JournalPointRole::Reflection,
                text: "a supported interpretation".into(),
                evidence: vec![],
            }],
            episodes: vec![episode.clone()],
            lineage: vec![],
        };
        assert!(
            CognitiveContent::Journal(journal.clone())
                .validate(&[])
                .is_err()
        );
        journal.points[0].evidence = vec![support(source)];
        let body = CognitiveContent::Journal(journal);
        body.validate(&[]).unwrap();
        assert!(body.references().contains(&episode));
    }
    #[test]
    fn preallocated_slot_dependencies_bound_model_proposals_without_invented_ids() {
        let a = StructuredTargetSlot {
            key: "first".into(),
            kind: CognitiveKind::Concept,
            id: CognitiveId::new(),
            expected_revision: 0,
        };
        let b = StructuredTargetSlot {
            key: "second".into(),
            kind: CognitiveKind::Concept,
            id: CognitiveId::new(),
            expected_revision: 0,
        };
        let source = material();
        let evidence = vec![support(source.clone())];
        let mut dependent = concept("dependent concept");
        if let CognitiveContent::Concept(value) = &mut dependent {
            value.members = vec![a.output_reference().unwrap()];
        }
        let mut proposal = StructuredProposal {
            groups: vec![
                StructuredProposalGroup {
                    key: "a".into(),
                    depends_on: vec![],
                    changes: vec![StructuredProposalChange {
                        slot_key: "first".into(),
                        body: concept("first concept"),
                        evidence: evidence.clone(),
                    }],
                },
                StructuredProposalGroup {
                    key: "b".into(),
                    depends_on: vec!["a".into()],
                    changes: vec![StructuredProposalChange {
                        slot_key: "second".into(),
                        body: dependent,
                        evidence,
                    }],
                },
            ],
        };
        proposal
            .validate_against(&[a.clone(), b.clone()], std::slice::from_ref(&source))
            .unwrap();
        proposal.groups[1].depends_on.clear();
        assert!(proposal.validate_against(&[a, b], &[source]).is_err());
    }
}

/// Scoped exact heads for atomic semantic maintenance operations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredExpectation {
    pub id: CognitiveId,
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeConceptsRequest {
    pub survivor: StructuredExpectation,
    pub retired: Vec<StructuredExpectation>,
    pub evidence: Vec<EvidenceLink>,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitConceptRequest {
    pub parent: StructuredExpectation,
    pub children: Vec<ConceptContent>,
    pub evidence: Vec<EvidenceLink>,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResegmentEpisodesRequest {
    pub sources: Vec<StructuredExpectation>,
    pub segments: Vec<EpisodeContent>,
    pub evidence: Vec<EvidenceLink>,
    pub reason: String,
}
impl StructuredExpectation {
    pub fn validate(&self) -> Result<()> {
        validate_uuid(self.id.as_str())?;
        positive_revision(self.expected_revision)
    }
}
impl MergeConceptsRequest {
    pub fn validate(&self) -> Result<()> {
        self.survivor.validate()?;
        text(&self.reason, "merge reason", 65536)?;
        validate_lineage_basis(&self.evidence)?;
        if !(1..=7).contains(&self.retired.len()) {
            return Err(Error::invalid("merge requires 1..7 retired concepts"));
        }
        let mut ids = BTreeSet::from([self.survivor.id.clone()]);
        for target in &self.retired {
            target.validate()?;
            if !ids.insert(target.id.clone()) {
                return Err(Error::invalid("duplicate merge concept"));
            }
        }
        Ok(())
    }
}
impl SplitConceptRequest {
    pub fn validate(&self) -> Result<()> {
        self.parent.validate()?;
        text(&self.reason, "split reason", 65536)?;
        validate_lineage_basis(&self.evidence)?;
        if !(2..=8).contains(&self.children.len()) {
            return Err(Error::invalid("split requires 2..8 child concepts"));
        }
        let mut aliases = BTreeSet::new();
        for child in &self.children {
            CognitiveContent::Concept(child.clone()).validate(&self.evidence)?;
            if child.canonical.is_some() {
                return Err(Error::invalid(
                    "split children must have their own canonical identity",
                ));
            }
            for key in child.alias_keys() {
                if !aliases.insert(key) {
                    return Err(Error::invalid("split children share a label or alias"));
                }
            }
        }
        Ok(())
    }
}
impl ResegmentEpisodesRequest {
    pub fn validate(&self) -> Result<()> {
        text(&self.reason, "resegmentation reason", 65536)?;
        validate_links(&self.evidence)?;
        if !(1..=8).contains(&self.sources.len()) || !(1..=16).contains(&self.segments.len()) {
            return Err(Error::invalid(
                "resegmentation requires 1..8 source episodes and 1..16 segments",
            ));
        }
        let mut ids = BTreeSet::new();
        for target in &self.sources {
            target.validate()?;
            if !ids.insert(target.id.clone()) {
                return Err(Error::invalid("duplicate resegmentation source"));
            }
        }
        let mut sources = BTreeSet::new();
        let mut count = 0;
        for segment in &self.segments {
            CognitiveContent::Episode(segment.clone()).validate(&self.evidence)?;
            for member in &segment.members {
                count += 1;
                if !sources.insert(member.source.clone()) {
                    return Err(Error::invalid("resegmentation repeats a source member"));
                }
            }
        }
        if count > 2048 {
            return Err(Error::invalid(
                "resegmentation exceeds 2048 ordered members",
            ));
        }
        Ok(())
    }
}
fn validate_lineage_basis(evidence: &[EvidenceLink]) -> Result<()> {
    if !(1..=16).contains(&evidence.len()) {
        return Err(Error::invalid(
            "concept lineage requires 1..16 exact source links",
        ));
    }
    validate_links(evidence)
}
