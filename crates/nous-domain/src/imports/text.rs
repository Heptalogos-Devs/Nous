use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportTextStrategy {
    #[default]
    Auto,
    Narrative,
    Factual,
    Quote,
    ChatLog,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImportTextSplitOptions {
    pub strategy: ImportTextStrategy,
    pub narrative_window_size: usize,
    pub narrative_overlap: usize,
    pub factual_target_size: usize,
    pub reference_time: Option<TimePoint>,
}
impl Default for ImportTextSplitOptions {
    fn default() -> Self {
        Self {
            strategy: ImportTextStrategy::Auto,
            narrative_window_size: 1600,
            narrative_overlap: 400,
            factual_target_size: 1200,
            reference_time: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImportTextSplitOverrides {
    pub strategy: Option<ImportTextStrategy>,
    pub narrative_window_size: Option<usize>,
    pub narrative_overlap: Option<usize>,
    pub factual_target_size: Option<usize>,
    pub reference_time: Option<TimePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportTextUnitDraft {
    pub ordinal: usize,
    pub start_byte: u64,
    pub end_byte: u64,
    pub text: String,
    pub effective_strategy: ImportTextStrategy,
    pub context: Value,
    pub requires_llm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewTextImportRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub document: ImportDocument,
    pub overrides: Option<ImportTextSplitOverrides>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextImportSplitWarning {
    pub code: String,
    pub message: String,
    pub ordinal: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TextImportPreview {
    pub content_hash: String,
    pub byte_len: u64,
    pub effective_options: ImportTextSplitOptions,
    pub selected_strategy: ImportTextStrategy,
    pub units: Vec<ImportTextUnitDraft>,
    pub warnings: Vec<TextImportSplitWarning>,
}
