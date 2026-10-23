//! Model resource roles and the provider call ledger.
use crate::{Error, ModelCallId, NamespaceId, SubjectId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    MemoryFormation,
    StructuredFormation,
    TextEmbedding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallState {
    Started,
    Succeeded,
    Failed,
    Uncertain,
}

/// Actual request/response payloads retain prompt content, source references,
/// parameters, usage and the provider's model identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCall {
    pub id: ModelCallId,
    pub job_id: String,
    pub step_key: String,
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
