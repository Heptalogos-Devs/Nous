use nous_domain::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AmemorixRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub operation: AmemorixOperation,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "tool",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AmemorixOperation {
    MemoryBundleAdmin(super::bundles::BundleInput),
    MemoryStats(StatsInput),
    MemoryRuntimeAdmin(RuntimeInput),
    SearchMemory(SearchInput),
    IngestText(IngestInput),
    IngestSummary(SummaryInput),
    MaintainMemory(MaintainInput),
    GetPersonProfile(ProfileInput),
    MemorySource(SourceInput),
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatsInput {}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeInput {
    pub action: String,
    #[serde(flatten)]
    pub parameters: serde_json::Map<String, Value>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Search,
    Time,
    Hybrid,
    Episode,
    Aggregate,
}
fn search_limit() -> usize {
    5
}
fn profile_limit() -> usize {
    10
}
fn maintain_limit() -> usize {
    50
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ExternalTime {
    Seconds(f64),
    Text(String),
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SearchInput {
    pub query: String,
    pub limit: usize,
    pub mode: SearchMode,
    pub chat_id: String,
    pub shared_chat_ids: Vec<String>,
    pub person_id: String,
    pub time_start: Option<ExternalTime>,
    pub time_end: Option<ExternalTime>,
    pub respect_filter: bool,
    pub user_id: String,
    pub group_id: String,
}
impl Default for SearchInput {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: search_limit(),
            mode: SearchMode::Search,
            chat_id: String::new(),
            shared_chat_ids: vec![],
            person_id: String::new(),
            time_start: None,
            time_end: None,
            respect_filter: true,
            user_id: String::new(),
            group_id: String::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationInput {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: Option<f64>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IngestInput {
    pub external_id: String,
    pub source_type: String,
    pub text: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default)]
    pub person_ids: Vec<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    pub timestamp: Option<ExternalTime>,
    pub time_start: Option<ExternalTime>,
    pub time_end: Option<ExternalTime>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub relations: Vec<RelationInput>,
    #[serde(default = "yes")]
    pub respect_filter: bool,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub group_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryInput {
    pub external_id: String,
    pub chat_id: String,
    pub text: String,
    #[serde(default)]
    pub participants: Vec<String>,
    pub time_start: Option<ExternalTime>,
    pub time_end: Option<ExternalTime>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
    #[serde(default = "yes")]
    pub respect_filter: bool,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub group_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaintainInput {
    pub operation_id: String,
    pub action: MaintainAction,
    #[serde(default)]
    pub target: String,
    pub hours: Option<f64>,
    #[serde(default)]
    pub reason: String,
    #[serde(default = "maintain_limit")]
    pub limit: usize,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaintainAction {
    Reinforce,
    Weaken,
    Protect,
    Unprotect,
    Freeze,
    Forget,
    Restore,
    Trash,
    Purge,
    Decay,
    RecycleBin,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    #[serde(default)]
    pub person_id: String,
    #[serde(default)]
    pub person_name: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default = "profile_limit")]
    pub limit: usize,
    #[serde(default)]
    pub context_text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceInput {
    pub target: RetrievalTarget,
}
