use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceAdminInput {
    pub action: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub requested_by: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteAdminInput {
    pub action: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub selector: Option<Value>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub operation_id: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub requested_by: String,
    #[serde(default = "limit")]
    pub limit: usize,
    pub before_commit_seq: Option<i64>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub cascade: bool,
    pub grace_hours: Option<f64>,
}
fn limit() -> usize {
    50
}
