use super::{
    MAX_CHANGES, MAX_GROUPS,
    dto::*,
    input::{StructuredFormationInput, validate_key},
};
use nous_domain as d;
use std::collections::{BTreeMap, BTreeSet};

impl RawEvidenceLink {
    fn compile(&self, input: &StructuredFormationInput) -> d::Result<d::EvidenceLink> {
        Ok(d::EvidenceLink {
            source: input.evidence_source(&self.source.0)?,
            polarity: self.polarity,
            relation: self.relation,
            explanation: self.explanation.clone(),
        })
    }
}

impl RawLineageLink {
    fn compile(&self, input: &StructuredFormationInput) -> d::Result<d::LineageLink> {
        Ok(d::LineageLink {
            relation: self.relation,
            target: input.reference(&self.target.0)?,
            reason: self.reason.clone(),
        })
    }
}

impl RawCognitiveContent {
    fn compile(&self, input: &StructuredFormationInput) -> d::Result<d::CognitiveContent> {
        let refs = |values: &[ReferenceKey]| {
            values
                .iter()
                .map(|key| input.reference(&key.0))
                .collect::<d::Result<Vec<_>>>()
        };
        let sources = |values: &[SourceKey]| {
            values
                .iter()
                .map(|key| input.evidence_source(&key.0))
                .collect::<d::Result<Vec<_>>>()
        };
        let lineage = |values: &[RawLineageLink]| {
            values
                .iter()
                .map(|value| value.compile(input))
                .collect::<d::Result<Vec<_>>>()
        };
        Ok(match self {
            Self::MemorySemantics(value) => {
                let memory = input.reference(&value.memory.0)?;
                if memory.kind != d::CognitiveKind::Memory {
                    return Err(d::Error::invalid(
                        "memory semantics must select an existing Memory revision",
                    ));
                }
                d::CognitiveContent::MemorySemantics(d::MemorySemantics {
                    memory_id: memory.id.parse()?,
                    memory_revision: memory.revision,
                    cognitive_role: value.cognitive_role,
                    formation_mode: value.formation_mode,
                    epistemic: value.epistemic.clone(),
                    grounding_observation: value
                        .grounding_observation
                        .as_ref()
                        .map(|key| input.observation(&key.0))
                        .transpose()?,
                    concepts: refs(&value.concepts)?,
                    qualifications: value.qualifications.clone(),
                })
            }
            Self::Concept(value) => d::CognitiveContent::Concept(d::ConceptContent {
                label: value.label.clone(),
                description: value.description.clone(),
                aliases: value.aliases.clone(),
                canonical: value
                    .canonical
                    .as_ref()
                    .map(|key| input.reference(&key.0))
                    .transpose()?,
                members: refs(&value.members)?,
                lineage: lineage(&value.lineage)?,
            }),
            Self::Episode(value) => d::CognitiveContent::Episode(d::EpisodeContent {
                title: value.title.clone(),
                track: value.track.clone(),
                experience: value.experience.clone(),
                parent: value
                    .parent
                    .as_ref()
                    .map(|key| input.reference(&key.0))
                    .transpose()?,
                members: value
                    .members
                    .iter()
                    .map(|member| {
                        Ok(d::EpisodeMember {
                            source: input.evidence_source(&member.source.0)?,
                            role: member.role.clone(),
                            ordinal: member.ordinal,
                        })
                    })
                    .collect::<d::Result<_>>()?,
                boundary_explanation: value.boundary_explanation.clone(),
                lineage: lineage(&value.lineage)?,
            }),
            Self::Journal(value) => d::CognitiveContent::Journal(d::JournalContent {
                title: value.title.clone(),
                narrative: value.narrative.clone(),
                period: value.period.clone(),
                knowledge_cutoff: input.knowledge_cutoff,
                perspective: value.perspective.clone(),
                episodes: refs(&value.episodes)?,
                lineage: lineage(&value.lineage)?,
                points: value
                    .points
                    .iter()
                    .map(|point| {
                        Ok(d::JournalPoint {
                            role: point.role,
                            text: point.text.clone(),
                            evidence: point
                                .evidence
                                .iter()
                                .map(|link| link.compile(input))
                                .collect::<d::Result<_>>()?,
                        })
                    })
                    .collect::<d::Result<_>>()?,
            }),
            Self::Schema(value) => d::CognitiveContent::Schema(d::SchemaContent {
                title: value.title.clone(),
                claim: value.claim.clone(),
                scope: d::SchemaScope {
                    description: value.scope.description.clone(),
                    about: refs(&value.scope.about)?,
                    concepts: refs(&value.scope.concepts)?,
                    validity: value.scope.validity.clone(),
                },
                boundary: value.boundary.clone(),
                formation_mode: value.formation_mode,
                epistemic: value.epistemic.clone(),
                support: sources(&value.support)?,
                counterexamples: sources(&value.counterexamples)?,
                boundary_cases: sources(&value.boundary_cases)?,
                lineage: lineage(&value.lineage)?,
            }),
            Self::Association(value) => d::CognitiveContent::Association(d::AssociationContent {
                from: input.reference(&value.from.0)?,
                to: input.reference(&value.to.0)?,
                relation: value.relation.clone(),
                polarity: value.polarity,
                basis_class: value.basis_class,
                description: value.description.clone(),
            }),
        })
    }
}

impl RawProposalGroup {
    /// Only this group is compiled. Its invalid references cannot erase other
    /// independent proposals. The app/transaction owner checks semantic validity,
    /// expected revisions, dependency outcomes and authoritative source scope.
    pub fn compile(
        &self,
        input: &StructuredFormationInput,
        proposals: &BTreeMap<String, Option<RawProposalChange>>,
    ) -> d::Result<d::StructuredProposalGroup> {
        input.validate()?;
        let changes = proposals
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_ref()
                    .filter(|change| change.group_key == self.key)
                    .map(|change| (key, change))
            })
            .map(|(slot_key, change)| {
                let slot = input
                    .slots
                    .iter()
                    .find(|slot| slot.key == *slot_key)
                    .ok_or_else(|| d::Error::invalid("proposal selected an unknown target slot"))?;
                let body = change.body.compile(input)?;
                if body.kind() != slot.kind {
                    return Err(d::Error::invalid(
                        "proposal body kind differs from its preallocated target slot",
                    ));
                }
                Ok(d::StructuredProposalChange {
                    slot_key: slot.key.clone(),
                    body,
                    evidence: change
                        .evidence
                        .iter()
                        .map(|link| link.compile(input))
                        .collect::<d::Result<_>>()?,
                })
            })
            .collect::<d::Result<Vec<_>>>()?;
        if changes.is_empty() || changes.len() > 16 {
            return Err(d::Error::invalid("proposal group needs 1 to 16 changes"));
        }
        Ok(d::StructuredProposalGroup {
            key: self.key.clone(),
            depends_on: self.depends_on.clone(),
            changes,
        })
    }
}

impl RawStructuredProposal {
    pub fn validate_envelope(&self) -> d::Result<()> {
        if self.groups.len() > MAX_GROUPS || self.changes.len() > MAX_CHANGES {
            return Err(d::Error::invalid(
                "structured proposal exceeds its group/change envelope",
            ));
        }
        let mut prior = BTreeSet::new();
        for group in &self.groups {
            validate_key(&group.key)?;
            let mut dependencies = BTreeSet::new();
            if prior.contains(&group.key)
                || group
                    .depends_on
                    .iter()
                    .any(|key| !prior.contains(key) || !dependencies.insert(key))
            {
                return Err(d::Error::invalid(
                    "structured groups need unique keys and distinct earlier dependencies",
                ));
            }
            prior.insert(group.key.clone());
        }
        for (slot_key, change) in &self.changes {
            validate_key(slot_key)?;
            if let Some(change) = change
                && !prior.contains(&change.group_key)
            {
                return Err(d::Error::invalid("proposal change has no declared group"));
            }
        }
        Ok(())
    }
}
