use crate::{context_work::WorkContextId, media::MediaReference, *};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceReadRef {
    Cognition {
        reference: CognitiveRef,
    },
    Media {
        reference: MediaReference,
        observation_id: Option<ObservationId>,
    },
    Observation {
        id: ObservationId,
    },
    WorkContext {
        id: WorkContextId,
    },
}
impl SourceReadRef {
    pub fn retrieval_target(&self) -> Option<RetrievalTarget> {
        match self {
            Self::Cognition { reference } => Some(RetrievalTarget::Cognition(reference.clone())),
            Self::Media { reference, .. } => Some(RetrievalTarget::Media(reference.clone())),
            _ => None,
        }
    }
    pub fn target(value: &RetrievalTarget) -> Self {
        match value {
            RetrievalTarget::Cognition(reference) => Self::Cognition {
                reference: reference.clone(),
            },
            RetrievalTarget::Media(reference) => Self::Media {
                reference: reference.clone(),
                observation_id: None,
            },
        }
    }
    pub fn evidence(value: &EvidenceSource) -> Self {
        match value {
            EvidenceSource::CognitiveRevision { reference } => Self::Cognition {
                reference: reference.clone(),
            },
            EvidenceSource::MemoryRevision {
                memory_id,
                revision,
            } => Self::Cognition {
                reference: CognitiveRef::memory(memory_id, *revision),
            },
            EvidenceSource::Material { material_id } => Self::Media {
                reference: MediaReference::Original {
                    material_id: material_id.clone(),
                },
                observation_id: None,
            },
            EvidenceSource::Observation { observation_id } => Self::Observation {
                id: observation_id.clone(),
            },
            EvidenceSource::Media { reference } => Self::Media {
                reference: reference.clone(),
                observation_id: None,
            },
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Cognition { reference } => reference.validate(),
            Self::Media {
                reference,
                observation_id,
            } => {
                reference.validate()?;
                if let Some(id) = observation_id {
                    id.validate()?;
                }
                Ok(())
            }
            Self::Observation { id } => id.validate(),
            Self::WorkContext { id } => id.validate(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceReadScope {
    pub subject_id: SubjectId,
    #[serde(default)]
    pub source_refs: Vec<SourceReadRef>,
    #[serde(default)]
    pub origin_conversation_refs: Vec<ConversationRef>,
}
impl SourceReadScope {
    pub fn canonical(&self) -> Self {
        let mut scope = self.clone();
        scope.source_refs.sort();
        scope.origin_conversation_refs.sort();
        scope
    }
    pub fn empty(subject_id: SubjectId) -> Self {
        Self {
            subject_id,
            source_refs: Vec::new(),
            origin_conversation_refs: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.subject_id.validate()?;
        if self.source_refs.len() > 1024 || self.origin_conversation_refs.len() > 64 {
            return Err(Error::invalid(
                "source-read selection exceeds its bounded metadata scope",
            ));
        }
        let mut refs = BTreeSet::new();
        for reference in &self.source_refs {
            reference.validate()?;
            if !refs.insert(reference) {
                return Err(Error::invalid("source-read references must be distinct"));
            }
        }
        let mut origins = BTreeSet::new();
        for origin in &self.origin_conversation_refs {
            origin.validate()?;
            if !origins.insert(origin) {
                return Err(Error::invalid("source-read origins must be distinct"));
            }
        }
        Ok(())
    }
}
