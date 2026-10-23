//! Persisted text embedding jobs and immutable model spaces.
use crate::{Error, MemoryId, ModelCallId, NamespaceId, SubjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingKind {
    Memory,
    Tag,
    Query,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingInputItem {
    pub kind: EmbeddingKind,
    pub source_key: String,
    pub text: String,
    pub memory_id: Option<MemoryId>,
    pub memory_revision: Option<i64>,
    pub source_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingState {
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
pub struct EmbeddingRun {
    pub job_id: String,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub producer_signature: String,
    pub space_id: String,
    pub inputs: Vec<EmbeddingInputItem>,
    pub state: EmbeddingState,
    pub model_call_id: Option<ModelCallId>,
    pub error: Option<Error>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub kind: EmbeddingKind,
    pub source_key: String,
    pub source_hash: String,
    pub memory_id: Option<MemoryId>,
    pub memory_revision: Option<i64>,
    pub vector: Vec<f32>,
    pub model_call_id: ModelCallId,
    pub commit_seq: i64,
}
