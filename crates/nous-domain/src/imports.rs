//! Durable document imports keep container provenance separate from admitted records.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod text;
pub use text::*;
mod catalog;
pub use catalog::*;
mod processing;
pub use processing::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    Json,
    Jsonl,
    Text,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ImportCompletion {
    #[default]
    Accepted,
    Formed,
}
impl ImportCompletion {
    fn is_accepted(&self) -> bool {
        *self == Self::Accepted
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub inputs: Vec<ImportDocument>,
    #[serde(default)]
    pub form_memory: bool,
    #[serde(default, skip_serializing_if = "ImportCompletion::is_accepted")]
    pub completion: ImportCompletion,
    pub origin: Option<crate::ObservationOrigin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportDocument {
    pub name: Option<String>,
    pub format: ImportFormat,
    pub input: ImportDocumentInput,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImportDocumentInput {
    Inline { content: String },
    Material { material_id: MaterialId },
}

identity!(ImportInputSourceId);
identity!(ImportRecordContainerId);

/// A local task directory references an independently retained input source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportInputRef {
    pub input_index: usize,
    pub source_id: ImportInputSourceId,
}

/// Exact input bytes and their real admission provenance, independent of local Tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportInputSource {
    pub id: ImportInputSourceId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub name: Option<String>,
    pub format: ImportFormat,
    pub material_id: MaterialId,
    pub content_hash: String,
    pub byte_len: u64,
    pub original_job_id: Option<String>,
    pub original_input_index: usize,
    pub recorded_at: String,
    pub commit_seq: i64,
}

/// An actual attempt's container, including attempts that replay an earlier admission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRecordContainer {
    pub id: ImportRecordContainerId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub input_source_id: ImportInputSourceId,
    pub record_index: usize,
    pub record_material_id: MaterialId,
    pub observation_id: ObservationId,
    pub memory_id: MemoryId,
    pub replayed: bool,
    pub original_job_id: String,
    pub original_attempt_index: usize,
    pub recorded_at: String,
    pub commit_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportContainerSource {
    pub input: ImportInputSource,
    pub container: ImportRecordContainer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRecord {
    pub external_id: String,
    pub text: String,
    pub source: Option<String>,
    pub source_type: Option<String>,
    pub event_time: Option<TimePoint>,
    pub observed_at: Option<TimePoint>,
    pub origin: Option<crate::ObservationOrigin>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
    pub structured: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedImportRecord {
    pub index: usize,
    pub input_index: usize,
    pub record_index: Option<usize>,
    pub external_id: Option<String>,
    pub record: Option<ImportRecord>,
    pub error: Option<Error>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportItemResult {
    pub index: usize,
    #[serde(default)]
    pub input_index: Option<usize>,
    #[serde(default)]
    pub record_index: Option<usize>,
    pub external_id: Option<String>,
    pub status: String,
    pub admission: Option<ImportAdmission>,
    pub commit_seq: Option<i64>,
    pub error: Option<Error>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub job_id: String,
    #[serde(default)]
    pub completion: ImportCompletion,
    #[serde(default)]
    pub inputs: Vec<ImportInputRef>,
    pub items: Vec<ImportItemResult>,
    pub accepted: usize,
    pub replayed: usize,
    pub rejected: usize,
}

/// Exact objects admitted by one faithful import record operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportAdmission {
    pub external_id: String,
    pub material_id: MaterialId,
    pub observation_id: ObservationId,
    pub memory_id: MemoryId,
    pub formation_job_id: Option<String>,
    pub structured_preserved: bool,
}
