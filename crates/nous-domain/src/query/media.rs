use crate::{media::*, *};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MediaRecord {
    Original(Material),
    SourceRegion(SourceRegion),
    Representation(Box<DerivedRepresentation>),
    DerivedRegion(DerivedRegion),
}
impl MediaRecord {
    pub fn reference(&self) -> MediaReference {
        match self {
            Self::Original(value) => MediaReference::Original {
                material_id: value.id.clone(),
            },
            Self::SourceRegion(value) => MediaReference::SourceRegion {
                id: value.id.clone(),
            },
            Self::Representation(value) => MediaReference::Representation {
                reference: value.reference(),
            },
            Self::DerivedRegion(value) => MediaReference::DerivedRegion {
                id: value.id.clone(),
            },
        }
    }
    pub fn namespace_id(&self) -> &NamespaceId {
        match self {
            Self::Original(value) => &value.namespace_id,
            Self::SourceRegion(value) => &value.namespace_id,
            Self::Representation(value) => &value.namespace_id,
            Self::DerivedRegion(value) => &value.namespace_id,
        }
    }
    pub fn subject_id(&self) -> Option<&SubjectId> {
        match self {
            Self::Representation(value) => Some(&value.subject_id),
            Self::DerivedRegion(value) => Some(&value.subject_id),
            Self::Original(_) | Self::SourceRegion(_) => None,
        }
    }
    pub fn commit_seq(&self) -> i64 {
        match self {
            Self::Original(value) => value.commit_seq,
            Self::SourceRegion(value) => value.commit_seq,
            Self::Representation(value) => value.commit_seq,
            Self::DerivedRegion(value) => value.commit_seq,
        }
    }
    pub fn recorded_at(&self) -> &str {
        match self {
            Self::Original(value) => &value.recorded_at,
            Self::SourceRegion(value) => &value.recorded_at,
            Self::Representation(value) => &value.recorded_at,
            Self::DerivedRegion(value) => &value.recorded_at,
        }
    }
    pub fn from_lineage(lineage: &MediaSourceLineage) -> Result<Self> {
        let value = match &lineage.source {
            MediaReference::Original { material_id } => lineage
                .materials
                .iter()
                .find(|value| value.material.id == *material_id)
                .map(|value| Self::Original(value.material.clone())),
            MediaReference::SourceRegion { id } => lineage
                .source_regions
                .iter()
                .find(|value| value.id == *id)
                .cloned()
                .map(Self::SourceRegion),
            MediaReference::Representation { reference } => lineage
                .representations
                .iter()
                .find(|value| value.reference() == *reference)
                .cloned()
                .map(|value| Self::Representation(Box::new(value))),
            MediaReference::DerivedRegion { id } => lineage
                .derived_regions
                .iter()
                .find(|value| value.id == *id)
                .cloned()
                .map(Self::DerivedRegion),
        };
        value.ok_or_else(|| Error::internal("media lineage does not contain its selected source"))
    }
}
