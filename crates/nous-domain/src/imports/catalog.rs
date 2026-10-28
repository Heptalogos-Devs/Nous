use crate::{media::SourceRegionId, *};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

identity!(ImportUnitCatalogId);
identity!(ImportUnitId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUnitCatalog {
    pub id: ImportUnitCatalogId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub parent_catalog_id: Option<ImportUnitCatalogId>,
    pub documents: Vec<ImportUnitDocumentPlan>,
    pub original_unit_count: usize,
    pub recorded_at: String,
    pub commit_seq: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUnitDocumentPlan {
    pub input_source_id: ImportInputSourceId,
    pub effective_options: ImportTextSplitOptions,
    pub selected_strategy: ImportTextStrategy,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportTextUnit {
    pub id: ImportUnitId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub catalog_id: ImportUnitCatalogId,
    pub ordinal: usize,
    pub input_source_id: ImportInputSourceId,
    pub source_region_id: SourceRegionId,
    pub text: String,
    pub effective_strategy: ImportTextStrategy,
    pub context: Value,
    pub context_region_ids: Vec<SourceRegionId>,
    pub recorded_at: String,
    pub commit_seq: i64,
}
impl ImportTextUnit {
    pub fn source_requirements(&self) -> Vec<SourceReadRef> {
        std::iter::once(self.source_region_id.clone())
            .chain(self.context_region_ids.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| SourceReadRef::Media {
                reference: media::MediaReference::SourceRegion { id },
                observation_id: None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FreezeImportCatalogRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub inputs: Vec<ImportDocument>,
    pub overrides: Option<ImportTextSplitOverrides>,
    pub parent_catalog_id: Option<ImportUnitCatalogId>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadImportCatalogRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub catalog_id: ImportUnitCatalogId,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryImportInputsRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub catalog_id: ImportUnitCatalogId,
    pub unit_ids: Vec<ImportUnitId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportCatalogView {
    pub catalog: ImportUnitCatalog,
    pub input_sources: Vec<ImportInputSource>,
    pub units: Vec<ImportTextUnit>,
    pub retained_unit_count: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUnitInputs {
    pub catalog: ImportUnitCatalog,
    pub input_sources: Vec<ImportInputSource>,
    pub units: Vec<ImportTextUnit>,
    pub source_refs: Vec<SourceReadRef>,
}
