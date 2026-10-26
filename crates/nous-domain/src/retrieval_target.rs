//! Stable retrieval identities name cognition revisions and real media locators.
use crate::{CognitiveRef, Error, Result, media::*};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    content = "reference",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RetrievalTarget {
    Cognition(CognitiveRef),
    Media(MediaReference),
}

impl RetrievalTarget {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Cognition(reference) => reference.validate(),
            Self::Media(reference) => reference.validate(),
        }
    }
    pub fn cognition(&self) -> Option<&CognitiveRef> {
        match self {
            Self::Cognition(reference) => Some(reference),
            Self::Media(_) => None,
        }
    }
    pub fn media(&self) -> Option<&MediaReference> {
        match self {
            Self::Media(reference) => Some(reference),
            Self::Cognition(_) => None,
        }
    }
    pub fn key(&self) -> Result<String> {
        self.validate()?;
        Ok(self.to_string())
    }
    pub fn identity_key(&self) -> Result<String> {
        self.validate()?;
        Ok(match self {
            Self::Cognition(reference) => format!(
                "{}:{}",
                serde_json::to_value(reference.kind)?
                    .as_str()
                    .expect("kind string"),
                reference.id
            ),
            Self::Media(MediaReference::Representation { reference }) => {
                format!("representation:{}", reference.id)
            }
            _ => self.to_string(),
        })
    }
}
impl fmt::Display for RetrievalTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cognition(reference) => write!(f, "{reference}"),
            Self::Media(MediaReference::Original { material_id }) => {
                write!(f, "material:{material_id}")
            }
            Self::Media(MediaReference::SourceRegion { id }) => write!(f, "source-region:{id}"),
            Self::Media(MediaReference::Representation { reference }) => {
                write!(f, "representation:{}@{}", reference.id, reference.revision)
            }
            Self::Media(MediaReference::DerivedRegion { id }) => write!(f, "derived-region:{id}"),
        }
    }
}
impl FromStr for RetrievalTarget {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let parsed = if let Some(id) = value.strip_prefix("material:") {
            Self::Media(MediaReference::Original {
                material_id: id.parse()?,
            })
        } else if let Some(id) = value.strip_prefix("source-region:") {
            Self::Media(MediaReference::SourceRegion { id: id.parse()? })
        } else if let Some(id) = value.strip_prefix("derived-region:") {
            Self::Media(MediaReference::DerivedRegion { id: id.parse()? })
        } else if let Some(exact) = value.strip_prefix("representation:") {
            let (id, revision) = exact
                .rsplit_once('@')
                .ok_or_else(|| Error::invalid("representation needs UUID@revision"))?;
            Self::Media(MediaReference::Representation {
                reference: RepresentationRef {
                    id: id.parse()?,
                    revision: revision.parse().map_err(|error| {
                        Error::invalid("invalid representation revision").with_cause(&error)
                    })?,
                },
            })
        } else {
            Self::Cognition(value.parse()?)
        };
        parsed.validate()?;
        Ok(parsed)
    }
}
