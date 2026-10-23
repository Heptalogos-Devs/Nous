//! Memory authority, formation lifecycle and query contracts.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDraft {
    pub title: String,
    pub content: String,
    pub epistemic_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TimeRange {
    pub domain: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CognitiveTimes {
    pub event: Option<TimePoint>,
    pub observation: Option<TimePoint>,
    pub formation: TimePoint,
    pub validity: Option<TimeRange>,
    pub recorded: TimePoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub title: String,
    pub content: String,
    pub epistemic_status: String,
    pub observation_id: ObservationId,
    pub material_id: MaterialId,
    pub model_call_id: Option<ModelCallId>,
    pub prompt_version: Option<String>,
    #[serde(default)]
    pub origin: MemoryOrigin,
    pub times: CognitiveTimes,
    pub recorded_at: String,
    pub deleted: bool,
    #[serde(default)]
    pub lifecycle: MemoryLifecycle,
    pub commit_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationState {
    Queued,
    WaitingForConfiguration,
    WaitingForBudget,
    Running,
    Succeeded,
    Failed,
    Uncertain,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationRun {
    pub job_id: String,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub observation_id: ObservationId,
    pub material_id: MaterialId,
    pub subject_revision: i64,
    pub material_hash: String,
    pub state: FormationState,
    pub model_call_id: Option<ModelCallId>,
    pub memory_ids: Vec<MemoryId>,
    pub error: Option<Error>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallState {
    Started,
    Succeeded,
    Failed,
    Uncertain,
}

/// Versioned request/response payloads retain actual model parameters,
/// rendered prompt, source references, usage and the provider's model identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCall {
    pub id: ModelCallId,
    pub job_id: String,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub attempt: i64,
    pub resource_id: String,
    pub request: Value,
    pub response: Option<Value>,
    pub state: ModelCallState,
    pub error: Option<Error>,
    pub cost_microunits: Option<i64>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeAxis {
    Event,
    Observation,
    Formation,
    Recorded,
    Validity,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TimeFilter {
    pub axis: TimeAxis,
    pub range: TimeRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    #[serde(default)]
    pub mode: QueryMode,
    pub subject_id: SubjectId,
    #[serde(default)]
    pub text: String,
    pub reference: Option<MemoryId>,
    pub source: Option<String>,
    pub time: Option<TimeFilter>,
    #[serde(default = "default_query_limit")]
    pub limit: usize,
    pub min_commit_seq: Option<i64>,
    #[serde(default)]
    pub include_inactive: bool,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub memory: Memory,
    pub score: f32,
    pub reference: String,
    pub source: Option<String>,
    pub material_id: MaterialId,
    pub observation_id: ObservationId,
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
    pub eligible_memories: usize,
    pub embedded_memories: usize,
    pub missing_tags: usize,
    pub provenance: Vec<EmbeddingProvenance>,
    pub diagnostics: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProvenance {
    pub space_id: String,
    pub source_key: String,
    pub source_hash: String,
    pub memory_revision: Option<i64>,
    pub model_call_id: ModelCallId,
    pub commit_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySource {
    pub memory: Memory,
    pub observation: Observation,
    pub material: MaterialContent,
    pub model_call: Option<ModelCall>,
    #[serde(default)]
    pub embedding_provenance: Vec<EmbeddingProvenance>,
}
