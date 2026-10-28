//! Named source groups are exact labels of actual admitted record materials.
use crate::*;
use serde::{Deserialize, Serialize};

pub fn material_source_name<'a>(
    material: &'a Material,
    details: &'a MaterialDetails,
) -> Option<&'a str> {
    let retained = if details.source_type.as_deref() == Some("a_memorix_bundle") {
        details
            .metadata
            .pointer("/a_memorix/source")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                details
                    .metadata
                    .pointer("/semantic_row/fields/source")
                    .and_then(serde_json::Value::as_str)
            })
    } else {
        None
    };
    retained
        .or(material.source.as_deref())
        .filter(|source| !source.is_empty())
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceDirectoryRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrashSourcesRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub sources: Vec<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub reason: String,
}
impl TrashSourcesRequest {
    pub fn validate(&self) -> Result<()> {
        self.namespace_id.validate()?;
        self.subject_id.validate()?;
        if self.sources.is_empty()
            || self.sources.len() > 1000
            || self
                .sources
                .iter()
                .any(|source| source.is_empty() || source.len() > 4096)
        {
            return Err(Error::invalid(
                "source selection requires 1..1000 nonempty exact source labels of at most 4096 bytes",
            ));
        }
        if self.reason.len() > 4096 {
            return Err(Error::invalid("source deletion reason exceeds 4096 bytes"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedSourceSummary {
    pub source: String,
    pub memory_count: usize,
    pub material_count: usize,
    pub observation_count: usize,
    pub protected_count: usize,
    pub last_updated: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDirectory {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub watermark: i64,
    pub items: Vec<NamedSourceSummary>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceGraphChanges {
    pub revised_entities: Vec<CognitiveRef>,
    pub revised_associations: Vec<CognitiveRef>,
    pub retired_associations: Vec<CognitiveRef>,
    pub retired_annotations: Vec<CognitiveRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashSourcesResult {
    pub sources: Vec<String>,
    pub matched_sources: Vec<String>,
    pub trashed_memories: Vec<CognitiveRef>,
    pub graph: SourceGraphChanges,
    pub affected_independent: Vec<CognitiveRef>,
    pub force: bool,
}
