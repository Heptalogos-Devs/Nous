use crate::*;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ImportVectorPolicy {
    Disabled,
    #[default]
    MetadataAllowed,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessImportUnitsRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub catalog_id: ImportUnitCatalogId,
    pub unit_ids: Option<Vec<ImportUnitId>>,
    pub llm_enabled: bool,
    pub extract_time: bool,
    #[serde(default)]
    pub vectors: ImportVectorPolicy,
}

identity!(ImportUnitOutputId);

/// Immutable adopted output, separate from the task that executed an attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUnitOutput {
    pub id: ImportUnitOutputId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub unit_id: ImportUnitId,
    pub attempt_job_id: String,
    pub memory_ids: Vec<MemoryId>,
    pub cognition: Vec<CognitiveRef>,
    pub semantic_call_id: Option<ModelCallId>,
    pub temporal_call_id: Option<ModelCallId>,
    pub event_descriptions: Vec<String>,
    pub event_range: Option<TimeRange>,
    pub temporal_error: Option<Error>,
    pub recorded_at: String,
    pub commit_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUnitJobRef {
    pub unit_id: ImportUnitId,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportProcessingResult {
    pub catalog_id: ImportUnitCatalogId,
    pub unit_jobs: Vec<ImportUnitJobRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUnitProcessResult {
    pub output_id: ImportUnitOutputId,
    pub vector_job_ids: Vec<String>,
    pub vector_error: Option<Error>,
}
