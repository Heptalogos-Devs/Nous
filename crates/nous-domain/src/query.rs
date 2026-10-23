//! One discovery contract for Memory and structured cognition.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CognitiveRecord {
    Memory(Memory),
    Structured(StructuredObject),
}

impl CognitiveRecord {
    pub fn is_active(&self) -> bool {
        match self {
            Self::Memory(value) => value.is_active(),
            Self::Structured(value) => value.is_active(),
        }
    }
    pub fn reference(&self) -> CognitiveRef {
        match self {
            Self::Memory(value) => CognitiveRef::memory(&value.id, value.revision),
            Self::Structured(value) => value.reference(),
        }
    }
    pub fn times(&self) -> &CognitiveTimes {
        match self {
            Self::Memory(value) => &value.times,
            Self::Structured(value) => &value.times,
        }
    }
    pub fn deleted(&self) -> bool {
        match self {
            Self::Memory(value) => value.deleted,
            Self::Structured(value) => value.deleted,
        }
    }
    pub fn namespace_id(&self) -> &NamespaceId {
        match self {
            Self::Memory(value) => &value.namespace_id,
            Self::Structured(value) => &value.namespace_id,
        }
    }
    pub fn subject_id(&self) -> &SubjectId {
        match self {
            Self::Memory(value) => &value.subject_id,
            Self::Structured(value) => &value.subject_id,
        }
    }
    pub fn commit_seq(&self) -> i64 {
        match self {
            Self::Memory(value) => value.commit_seq,
            Self::Structured(value) => value.commit_seq,
        }
    }
    pub fn recorded_at(&self) -> &str {
        match self {
            Self::Memory(value) => &value.recorded_at,
            Self::Structured(value) => &value.recorded_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    #[serde(default)]
    pub mode: QueryMode,
    pub subject_id: SubjectId,
    #[serde(default)]
    pub text: String,
    pub reference: Option<CognitiveRef>,
    #[serde(default)]
    pub kinds: Vec<CognitiveKind>,
    pub source: Option<String>,
    pub time: Option<TimeFilter>,
    #[serde(default = "default_query_limit")]
    pub limit: usize,
    pub min_commit_seq: Option<i64>,
    pub as_of_commit: Option<i64>,
    #[serde(default)]
    pub include_inactive: bool,
    #[serde(default)]
    pub include_deleted: bool,
}

impl QueryRequest {
    pub fn validate(&self) -> Result<()> {
        if !(1..=100).contains(&self.limit) || self.text.len() > 4096 {
            return Err(Error::invalid(
                "query limit must be 1..100 and text at most 4096 bytes",
            ));
        }
        if let Some(reference) = &self.reference {
            reference.validate()?;
        }
        if let Some(time) = &self.time {
            time.validate()?;
        }
        let mut kinds = std::collections::BTreeSet::new();
        if self.kinds.iter().any(|kind| !kinds.insert(kind)) {
            return Err(Error::invalid("query kinds must be unique"));
        }
        if self.as_of_commit.is_some_and(|value| value < 0)
            || self.min_commit_seq.is_some_and(|value| value < 0)
        {
            return Err(Error::invalid("query commit bounds must be nonnegative"));
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QueryMode {
    #[default]
    Lexical,
    Dense,
    Vcp,
    Hybrid,
}
fn default_query_limit() -> usize {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryHit {
    pub reference: CognitiveRef,
    pub object: CognitiveRecord,
    pub score: f32,
    pub source_roots: Vec<MaterialId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCoverage {
    pub indexed_through: i64,
    pub authority_commit_seq: i64,
    pub complete: bool,
    pub generation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub hits: Vec<QueryHit>,
    pub coverage: QueryCoverage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<SemanticQueryInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticQueryInfo {
    pub mode: QueryMode,
    pub space_id: String,
    pub query_embedding_call_id: ModelCallId,
    pub generation: String,
    pub eligible_objects: usize,
    pub embedded_objects: usize,
    pub missing_tags: usize,
    pub provenance: Vec<EmbeddingProvenance>,
    pub diagnostics: Value,
}
