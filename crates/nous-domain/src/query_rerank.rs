use crate::{Error, ModelCallId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryRerankMode {
    #[default]
    Off,
    Optional,
    Required,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryRerankStatus {
    Skipped,
    Completed,
    Fallback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRerankInfo {
    pub mode: QueryRerankMode,
    pub status: QueryRerankStatus,
    pub job_id: Option<String>,
    pub model_call_id: Option<ModelCallId>,
    pub submitted_documents: usize,
    pub scored_documents: usize,
    pub retained_base_order: usize,
    pub reason: Option<String>,
    pub error: Option<Error>,
}
