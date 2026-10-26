//! Model resource roles and the provider call ledger.
use crate::{Error, ModelCallId, NamespaceId, SubjectId, TraceId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallState {
    Started,
    Succeeded,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelCallOrigin {
    Local,
    Imported {
        installation_id: String,
        original_call_id: ModelCallId,
        original_namespace_id: NamespaceId,
        original_subject_id: SubjectId,
    },
}

/// Actual request/response payloads retain prompt content, source references,
/// parameters, usage and the provider's model identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCall {
    pub id: ModelCallId,
    pub origin: ModelCallOrigin,
    pub trace_id: TraceId,
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
    pub usage: Option<crate::ModelUsage>,
    pub cost: Option<crate::ModelCost>,
    pub created_at: String,
    pub completed_at: Option<String>,
}
