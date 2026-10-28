use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

identity!(ImportFileSuccessId);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportFileDedupePolicy {
    None,
    #[default]
    ContentHash,
    Manifest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImportManifestKey {
    ContentHash { hash: String },
    CallerFile { key: String },
}
impl ImportFileDedupePolicy {
    pub fn key(self, content_hash: &str, file_identity: Option<&str>) -> Option<ImportManifestKey> {
        match (self, file_identity) {
            (Self::None, _) => None,
            (Self::Manifest, Some(key)) => Some(ImportManifestKey::CallerFile { key: key.into() }),
            _ => Some(ImportManifestKey::ContentHash {
                hash: content_hash.into(),
            }),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportFileDocument {
    pub document: ImportDocument,
    pub file_identity: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdmitImportFilesRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub inputs: Vec<ImportFileDocument>,
    pub overrides: Option<ImportTextSplitOverrides>,
    pub llm_enabled: bool,
    pub extract_time: bool,
    #[serde(default)]
    pub vectors: ImportVectorPolicy,
    pub origin: Option<ObservationOrigin>,
    #[serde(default)]
    pub dedupe_policy: ImportFileDedupePolicy,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub clear_manifest: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFileUnitOutput {
    pub unit_id: ImportUnitId,
    pub output_id: ImportUnitOutputId,
    pub vectors: ImportVectorPolicy,
}
/// Historical complete file processing, independent of local Job retention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFileSuccess {
    pub id: ImportFileSuccessId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub input_source_id: ImportInputSourceId,
    pub catalog_id: ImportUnitCatalogId,
    pub file_identity: Option<String>,
    pub origin: Option<ObservationOrigin>,
    pub unit_outputs: Vec<ImportFileUnitOutput>,
    pub dedupe_policy: ImportFileDedupePolicy,
    pub processing_job_id: String,
    pub recorded_at: String,
    pub commit_seq: i64,
}
/// Current qualification only; clearing a head never erases the success fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportManifestHead {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub origin: Option<ObservationOrigin>,
    pub policy: ImportFileDedupePolicy,
    pub key: ImportManifestKey,
    pub success_id: ImportFileSuccessId,
    pub recorded_at: String,
    pub commit_seq: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportFileAdmissionDecision {
    Process,
    Skip { success_id: ImportFileSuccessId },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFileAdmission {
    pub input_index: usize,
    pub input_source_id: ImportInputSourceId,
    pub decision: ImportFileAdmissionDecision,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFilesAdmission {
    pub inputs: Vec<ImportFileAdmission>,
    pub catalog_id: Option<ImportUnitCatalogId>,
    pub processing_job_id: Option<String>,
}
