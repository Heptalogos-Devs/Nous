//! Evidence polarity and recursive typed source enumeration.
use super::*;

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

impl CognitiveContent {
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
}

pub(super) fn source_ref(reference: &CognitiveRef) -> EvidenceSource {
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
pub(super) fn dedup_links(links: Vec<EvidenceLink>) -> Vec<EvidenceLink> {
    let mut seen = BTreeSet::new();
    links
        .into_iter()
        .filter(|link| seen.insert((link.source.clone(), link.polarity, link.relation)))
        .collect()
}
