//! Typed joint embedding inputs, immutable semantic spaces and accepted vectors.
use crate::{
    CognitiveRef, Error, JobRecord, Memory, ModelCallId, NamespaceId, Result, RetrievalTarget,
    StructuredObject, SubjectId, media::*,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
mod projection;
pub use projection::cognitive_embedding_text;
mod space;
pub use space::*;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    content = "target",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EmbeddingTarget {
    Item(RetrievalTarget),
    Tag,
    Query,
}
impl EmbeddingTarget {
    pub fn item(&self) -> Option<&RetrievalTarget> {
        match self {
            Self::Item(target) => Some(target),
            _ => None,
        }
    }
    pub fn source_key(&self, source_hash: &str) -> Result<String> {
        validate_hash(source_hash)?;
        match self {
            Self::Item(target) => target.key(),
            Self::Tag => Ok(format!("tag:{source_hash}")),
            Self::Query => Ok(format!("query:{source_hash}")),
        }
    }
    pub fn identity_key(&self, source_hash: &str) -> Result<String> {
        match self {
            Self::Item(target) => target.identity_key(),
            _ => self.source_key(source_hash),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmbeddingPart {
    Text {
        text: String,
        source: Option<MediaReference>,
    },
    Media {
        source: MediaReference,
        blob: BlobReference,
        channels: ChannelAccess,
    },
}
impl EmbeddingPart {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Text { text, .. } if text.trim().is_empty() || text.contains('\0') => Err(
                Error::invalid("embedding text must be nonempty and contain no NUL"),
            ),
            Self::Text {
                source: Some(source),
                ..
            } => source.validate(),
            Self::Text { .. } => Ok(()),
            Self::Media { source, blob, .. } => {
                source.validate()?;
                blob.validate()
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingInputItem {
    pub target: EmbeddingTarget,
    pub parts: Vec<EmbeddingPart>,
}
impl EmbeddingInputItem {
    pub fn for_memory(value: &Memory) -> Result<Self> {
        Self::text(
            EmbeddingTarget::Item(RetrievalTarget::Cognition(CognitiveRef::memory(
                &value.id,
                value.revision,
            ))),
            format!("{}\n{}", value.title, value.content),
        )
    }
    pub fn for_cognition(value: &StructuredObject, base_memory: Option<&Memory>) -> Result<Self> {
        Self::text(
            EmbeddingTarget::Item(RetrievalTarget::Cognition(value.reference())),
            cognitive_embedding_text(value, base_memory)?,
        )
    }
    pub fn for_media(reference: MediaReference, parts: Vec<EmbeddingPart>) -> Result<Self> {
        Self::new(
            EmbeddingTarget::Item(RetrievalTarget::Media(reference)),
            parts,
        )
    }
    pub fn tag(text: impl AsRef<str>) -> Result<Self> {
        Self::text(EmbeddingTarget::Tag, text.as_ref().trim().to_owned())
    }
    pub fn query(text: impl AsRef<str>) -> Result<Self> {
        Self::text(EmbeddingTarget::Query, text.as_ref().trim().to_owned())
    }
    pub fn query_parts(parts: Vec<EmbeddingPart>) -> Result<Self> {
        Self::new(EmbeddingTarget::Query, parts)
    }
    fn text(target: EmbeddingTarget, text: String) -> Result<Self> {
        Self::new(target, vec![EmbeddingPart::Text { text, source: None }])
    }
    pub fn new(target: EmbeddingTarget, parts: Vec<EmbeddingPart>) -> Result<Self> {
        let value = Self { target, parts };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.parts.is_empty() {
            return Err(Error::invalid("embedding input needs at least one part"));
        }
        for part in &self.parts {
            part.validate()?;
        }
        if let EmbeddingTarget::Item(target) = &self.target {
            target.validate()?;
        }
        if self.target == EmbeddingTarget::Tag && self.text_content().is_none() {
            return Err(Error::invalid("tag embedding requires one text part"));
        }
        if matches!(self.target, EmbeddingTarget::Tag | EmbeddingTarget::Query)
            && self.parts.iter().any(
                |part| matches!(part,EmbeddingPart::Text{text,source:None} if text!=text.trim()),
            )
        {
            return Err(Error::invalid(
                "free tag and query text parts must be trimmed",
            ));
        }
        Ok(())
    }
    pub fn text_content(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [EmbeddingPart::Text { text, .. }] => Some(text),
            _ => None,
        }
    }
    pub fn byte_len(&self) -> Result<usize> {
        self.parts.iter().try_fold(0usize, |sum, part| {
            let length = match part {
                EmbeddingPart::Text { text, .. } => text.len(),
                EmbeddingPart::Media { blob, .. } => {
                    usize::try_from(blob.byte_len).map_err(|_| {
                        Error::invalid("embedding artifact length exceeds platform range")
                    })?
                }
            };
            sum.checked_add(length)
                .ok_or_else(|| Error::invalid("embedding input length overflow"))
        })
    }
    pub fn source_hash(&self) -> String {
        let semantics = self
            .parts
            .iter()
            .map(|part| match part {
                EmbeddingPart::Text { text, .. } => json!({"kind":"text","text":text}),
                EmbeddingPart::Media { blob, channels, .. } => {
                    json!({"kind":"media","blob":blob,"channels":channels})
                }
            })
            .collect::<Vec<_>>();
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&semantics).expect("embedding semantics serialize"))
        )
    }
    pub fn source_key(&self) -> Result<String> {
        self.validate()?;
        self.target.source_key(&self.source_hash())
    }
}
pub fn cognitive_embedding_key(reference: &CognitiveRef) -> Result<String> {
    RetrievalTarget::Cognition(reference.clone()).key()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRun {
    pub job: JobRecord,
    pub space_id: String,
    pub inputs: Vec<EmbeddingInputItem>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRecord {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub space_id: String,
    pub target: EmbeddingTarget,
    pub source_hash: String,
    pub vector: Vec<f32>,
    pub model_call_id: ModelCallId,
    pub commit_seq: i64,
}
impl EmbeddingRecord {
    pub fn source_key(&self) -> Result<String> {
        self.target.source_key(&self.source_hash)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProvenance {
    pub space_id: String,
    pub target: EmbeddingTarget,
    pub source_hash: String,
    pub model_call_id: ModelCallId,
    pub commit_seq: i64,
}
impl EmbeddingProvenance {
    pub fn source_key(&self) -> Result<String> {
        self.target.source_key(&self.source_hash)
    }
}
fn validate_hash(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::invalid(
            "embedding source hash must be lowercase SHA-256",
        ));
    }
    Ok(())
}
