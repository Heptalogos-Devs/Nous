use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubjectDiagnosticsRequest {
    pub subject_id: SubjectId,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LifecycleCounts {
    pub current: u64,
    pub active: u64,
    pub inactive: u64,
    pub deleted: u64,
    pub withdrawn: u64,
    pub revisions: u64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubjectInventory {
    pub cognition: BTreeMap<String, LifecycleCounts>,
    pub media: BTreeMap<String, u64>,
    pub observations: BTreeMap<String, u64>,
    pub purged_memories: u64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkDiagnostics {
    pub by_kind: BTreeMap<String, BTreeMap<String, u64>>,
    pub waiting_by_reason: BTreeMap<String, u64>,
    pub queue_rows_by_status: BTreeMap<String, u64>,
    pub jobs_without_queue_row: u64,
    pub last_maintenance_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSpaceDiagnostics {
    pub space: EmbeddingSpace,
    pub contents: u64,
    pub associations: u64,
    pub retired_associations: u64,
    pub pending_inputs: u64,
    pub input_states: BTreeMap<String, BTreeMap<String, u64>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTotal {
    pub known: Option<u64>,
    pub contributing_calls: u64,
    pub missing_calls: u64,
    pub overflow: bool,
}
impl Default for TokenTotal {
    fn default() -> Self {
        Self {
            known: Some(0),
            contributing_calls: 0,
            missing_calls: 0,
            overflow: false,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostTotals {
    pub by_currency: BTreeMap<String, DecimalAmount>,
    pub missing_calls: u64,
    pub arithmetic_issues: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsageTotals {
    pub calls: u64,
    pub by_state: BTreeMap<String, u64>,
    pub by_kind: BTreeMap<String, u64>,
    pub input_tokens: TokenTotal,
    pub output_tokens: TokenTotal,
    pub total_tokens: TokenTotal,
    pub estimated_cost: CostTotals,
    pub reported_cost: CostTotals,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubjectModelDiagnostics {
    pub local: ModelUsageTotals,
    pub imported_history: ModelUsageTotals,
    pub imported_records: u64,
    pub imported_duplicate_records: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectDiagnostics {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub watermark: i64,
    pub inventory: SubjectInventory,
    pub work: WorkDiagnostics,
    pub vector_spaces: Vec<VectorSpaceDiagnostics>,
    pub models: SubjectModelDiagnostics,
    pub runtime: Value,
    pub unknown: BTreeMap<String, String>,
}
