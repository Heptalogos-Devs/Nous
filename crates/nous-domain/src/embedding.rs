//! Typed embedding inputs, immutable model spaces and accepted vectors.
use crate::{
    CognitiveRef, Error, JobRecord, Memory, ModelCallId, NamespaceId, Result, StructuredObject,
    SubjectId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod projection;
pub use projection::cognitive_embedding_text;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    content = "reference",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EmbeddingTarget {
    Cognition(CognitiveRef),
    Tag,
    Query,
}

impl EmbeddingTarget {
    pub fn reference(&self) -> Option<&CognitiveRef> {
        match self {
            Self::Cognition(reference) => Some(reference),
            Self::Tag | Self::Query => None,
        }
    }

    pub fn source_key(&self, source_hash: &str) -> Result<String> {
        validate_hash(source_hash)?;
        match self {
            Self::Cognition(reference) => cognitive_embedding_key(reference),
            Self::Tag => Ok(format!("tag:{source_hash}")),
            Self::Query => Ok(format!("query:{source_hash}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingInputItem {
    pub target: EmbeddingTarget,
    pub text: String,
}

impl EmbeddingInputItem {
    pub fn for_memory(value: &Memory) -> Result<Self> {
        Self::new(
            EmbeddingTarget::Cognition(CognitiveRef::memory(&value.id, value.revision)),
            format!("{}\n{}", value.title, value.content),
        )
    }

    pub fn for_cognition(value: &StructuredObject, base_memory: Option<&Memory>) -> Result<Self> {
        Self::new(
            EmbeddingTarget::Cognition(value.reference()),
            cognitive_embedding_text(value, base_memory)?,
        )
    }

    pub fn tag(text: impl AsRef<str>) -> Result<Self> {
        Self::new(EmbeddingTarget::Tag, text.as_ref().trim().to_owned())
    }

    pub fn query(text: impl AsRef<str>) -> Result<Self> {
        Self::new(EmbeddingTarget::Query, text.as_ref().trim().to_owned())
    }

    fn new(target: EmbeddingTarget, text: String) -> Result<Self> {
        let value = Self { target, text };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        if self.text.trim().is_empty() || self.text.contains('\0') {
            return Err(Error::invalid(
                "embedding text must be nonempty and contain no NUL",
            ));
        }
        match &self.target {
            EmbeddingTarget::Cognition(reference) => reference.validate(),
            EmbeddingTarget::Tag | EmbeddingTarget::Query if self.text != self.text.trim() => Err(
                Error::invalid("tag and query embedding text must be trimmed"),
            ),
            EmbeddingTarget::Tag | EmbeddingTarget::Query => Ok(()),
        }
    }

    pub fn source_hash(&self) -> String {
        format!("{:x}", Sha256::digest(self.text.as_bytes()))
    }

    pub fn source_key(&self) -> Result<String> {
        self.validate()?;
        self.target.source_key(&self.source_hash())
    }
}

/// The exact typed revision owns the key; text hashes identify reusable content.
pub fn cognitive_embedding_key(reference: &CognitiveRef) -> Result<String> {
    reference.validate()?;
    Ok(reference.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRun {
    pub job: JobRecord,
    pub producer_signature: String,
    pub space_id: String,
    pub inputs: Vec<EmbeddingInputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EmbeddingSpace {
    pub id: String,
    pub resource_id: String,
    pub provider_model: String,
    pub dimension: usize,
    pub producer_signature: String,
    pub modality: String,
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
    pub fn reference(&self) -> Option<&CognitiveRef> {
        self.target.reference()
    }

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
