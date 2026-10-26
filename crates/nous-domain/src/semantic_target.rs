use crate::*;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct CognitiveIdentity {
    pub kind: CognitiveKind,
    pub id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticTarget {
    Object { identity: CognitiveIdentity },
    Exact { reference: RetrievalTarget },
}
impl SemanticTarget {
    pub fn from_reference(reference: RetrievalTarget) -> Self {
        match reference {
            RetrievalTarget::Cognition(reference) => Self::Object {
                identity: CognitiveIdentity {
                    kind: reference.kind,
                    id: reference.id,
                },
            },
            reference => Self::Exact { reference },
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Object { identity } => crate::identity::validate_uuid(&identity.id),
            Self::Exact { reference } => reference.validate(),
        }
    }
    pub fn evidence_source(&self) -> Option<EvidenceSource> {
        match self {
            Self::Object { .. } => None,
            Self::Exact {
                reference: RetrievalTarget::Media(reference),
            } => Some(EvidenceSource::Media {
                reference: reference.clone(),
            }),
            Self::Exact {
                reference: RetrievalTarget::Cognition(reference),
            } if reference.kind == CognitiveKind::Memory => Some(EvidenceSource::MemoryRevision {
                memory_id: MemoryId(reference.id.clone()),
                revision: reference.revision,
            }),
            Self::Exact {
                reference: RetrievalTarget::Cognition(reference),
            } => Some(EvidenceSource::CognitiveRevision {
                reference: reference.clone(),
            }),
        }
    }
}
impl CognitiveContent {
    pub fn semantic_targets(&self) -> Vec<&SemanticTarget> {
        match self {
            Self::Concept(content) => content.members.iter().collect(),
            Self::Entity(content) => content
                .mentions
                .iter()
                .map(|mention| &mention.target)
                .collect(),
            _ => Vec::new(),
        }
    }
}
