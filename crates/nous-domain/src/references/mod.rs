//! Human-readable secondary addresses; canonical identity and authority stay with the owner.
mod package;
mod proquint;
use crate::{media::MediaReference, *};
pub use proquint::{decode_proquint, encode_proquint};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct LexicalRef(String);
impl LexicalRef {
    pub fn allocate(target: &SemanticTarget, bits: u64) -> Result<Self> {
        target.validate()?;
        format!("{}:{}", prefix(target), encode_proquint(bits)?).parse()
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl FromStr for LexicalRef {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let (prefix, body) = value
            .split_once(':')
            .ok_or_else(|| Error::invalid("readable reference requires a type prefix"))?;
        if !matches!(
            prefix,
            "mem"
                | "memrev"
                | "sem"
                | "semrev"
                | "tag"
                | "tagrev"
                | "ent"
                | "entrev"
                | "ep"
                | "eprev"
                | "journal"
                | "journalrev"
                | "schema"
                | "schemarev"
                | "assoc"
                | "assocrev"
                | "art"
                | "src"
                | "repr"
                | "region"
        ) {
            return Err(Error::invalid("unknown readable reference type"));
        }
        decode_proquint(body)?;
        Ok(Self(value.into()))
    }
}
impl fmt::Display for LexicalRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl TryFrom<String> for LexicalRef {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        value.parse()
    }
}
impl From<LexicalRef> for String {
    fn from(value: LexicalRef) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReferenceAddresses {
    pub object: Option<LexicalRef>,
    pub exact: LexicalRef,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceBinding {
    pub address: LexicalRef,
    pub target: SemanticTarget,
}
impl ReferenceBinding {
    pub fn validate(&self) -> Result<()> {
        self.target.validate()?;
        if self.address.as_str().split_once(':').map(|value| value.0) != Some(prefix(&self.target))
        {
            return Err(Error::invalid(
                "readable reference type differs from its canonical target",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ReferenceLocator {
    Canonical(RetrievalTarget),
    Readable(LexicalRef),
}
impl From<RetrievalTarget> for ReferenceLocator {
    fn from(value: RetrievalTarget) -> Self {
        Self::Canonical(value)
    }
}
impl FromStr for ReferenceLocator {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        if let Ok(reference) = value.parse::<RetrievalTarget>() {
            return Ok(Self::Canonical(reference));
        }
        Ok(Self::Readable(value.parse()?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReferenceResolution {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub reference: RetrievalTarget,
    pub addresses: Option<ReferenceAddresses>,
}

fn cognition_prefix(kind: CognitiveKind, exact: bool) -> &'static str {
    match (kind, exact) {
        (CognitiveKind::Memory, false) => "mem",
        (CognitiveKind::Memory, true) => "memrev",
        (CognitiveKind::MemorySemantics, false) => "sem",
        (CognitiveKind::MemorySemantics, true) => "semrev",
        (CognitiveKind::Concept, false) => "tag",
        (CognitiveKind::Concept, true) => "tagrev",
        (CognitiveKind::Entity, false) => "ent",
        (CognitiveKind::Entity, true) => "entrev",
        (CognitiveKind::Episode, false) => "ep",
        (CognitiveKind::Episode, true) => "eprev",
        (CognitiveKind::Journal, false) => "journal",
        (CognitiveKind::Journal, true) => "journalrev",
        (CognitiveKind::Schema, false) => "schema",
        (CognitiveKind::Schema, true) => "schemarev",
        (CognitiveKind::Association, false) => "assoc",
        (CognitiveKind::Association, true) => "assocrev",
    }
}
fn prefix(target: &SemanticTarget) -> &'static str {
    match target {
        SemanticTarget::Object { identity } => cognition_prefix(identity.kind, false),
        SemanticTarget::Exact {
            reference: RetrievalTarget::Cognition(reference),
        } => cognition_prefix(reference.kind, true),
        SemanticTarget::Exact {
            reference: RetrievalTarget::Media(reference),
        } => match reference {
            MediaReference::Original { .. } => "art",
            MediaReference::SourceRegion { .. } => "src",
            MediaReference::Representation { .. } => "repr",
            MediaReference::DerivedRegion { .. } => "region",
        },
    }
}
