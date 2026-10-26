//! Discovery over cognition and exact media, with one retrieval identity.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod media;
pub use media::MediaRecord;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RetrievalRecord {
    Memory(Memory),
    Structured(StructuredObject),
    Media(MediaRecord),
}

impl RetrievalRecord {
    pub fn is_active(&self) -> bool {
        match self {
            Self::Memory(value) => value.is_active(),
            Self::Structured(value) => value.is_active(),
            Self::Media(_) => true,
        }
    }
    pub fn reference(&self) -> RetrievalTarget {
        match self {
            Self::Memory(value) => {
                RetrievalTarget::Cognition(CognitiveRef::memory(&value.id, value.revision))
            }
            Self::Structured(value) => RetrievalTarget::Cognition(value.reference()),
            Self::Media(value) => RetrievalTarget::Media(value.reference()),
        }
    }
    pub fn cognitive_times(&self) -> Option<&CognitiveTimes> {
        match self {
            Self::Memory(value) => Some(&value.times),
            Self::Structured(value) => Some(&value.times),
            Self::Media(_) => None,
        }
    }
    pub fn deleted(&self) -> bool {
        match self {
            Self::Memory(value) => value.deleted,
            Self::Structured(value) => value.deleted,
            Self::Media(_) => false,
        }
    }
    pub fn namespace_id(&self) -> &NamespaceId {
        match self {
            Self::Memory(value) => &value.namespace_id,
            Self::Structured(value) => &value.namespace_id,
            Self::Media(value) => value.namespace_id(),
        }
    }
    pub fn subject_id(&self) -> Option<&SubjectId> {
        match self {
            Self::Memory(value) => Some(&value.subject_id),
            Self::Structured(value) => Some(&value.subject_id),
            Self::Media(value) => value.subject_id(),
        }
    }
    pub fn commit_seq(&self) -> i64 {
        match self {
            Self::Memory(value) => value.commit_seq,
            Self::Structured(value) => value.commit_seq,
            Self::Media(value) => value.commit_seq(),
        }
    }
    pub fn recorded_at(&self) -> &str {
        match self {
            Self::Memory(value) => &value.recorded_at,
            Self::Structured(value) => &value.recorded_at,
            Self::Media(value) => value.recorded_at(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct QueryInput {
    pub text: String,
    pub media: Vec<crate::media::MediaReference>,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QueryDomain {
    Cognition,
    Media,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    #[serde(default)]
    pub mode: QueryMode,
    pub subject_id: SubjectId,
    #[serde(default)]
    pub input: QueryInput,
    pub reference: Option<RetrievalTarget>,
    #[serde(default)]
    pub domains: Vec<QueryDomain>,
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
        if self.limit == 0 || self.input.text.contains('\0') {
            return Err(Error::invalid(
                "query limit must be positive and text may not contain NUL",
            ));
        }
        let mut media = std::collections::BTreeSet::new();
        for reference in &self.input.media {
            reference.validate()?;
            if !media.insert(reference) {
                return Err(Error::invalid("query media references must be unique"));
            }
        }
        let domains: std::collections::BTreeSet<_> = self.domains.iter().collect();
        if domains.len() != self.domains.len() {
            return Err(Error::invalid("query domains must be unique"));
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
    pub fn wants_domain(&self, domain: QueryDomain) -> bool {
        self.domains.is_empty() || self.domains.contains(&domain)
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
    pub reference: RetrievalTarget,
    pub object: RetrievalRecord,
    pub score: f32,
    pub source_roots: Vec<MaterialId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCoverage {
    pub indexed_through: i64,
    pub authority_commit_seq: i64,
    pub complete: bool,
    pub generation: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projection_issues: Vec<ProjectionIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionIssue {
    pub target: RetrievalTarget,
    pub error: Error,
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
