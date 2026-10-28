use nous_domain::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BundleInput {
    Export(ExportInput),
    Inspect {
        material_id: MaterialId,
    },
    Import(ImportInput),
    List {
        #[serde(default = "limit")]
        limit: usize,
    },
    ResolveFile {
        file_name: String,
    },
    Uninstall {
        installation_id: String,
        #[serde(default = "operation_id")]
        operation_id: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PackageInput {
    pub id: String,
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {
    #[serde(default = "operation_id")]
    pub operation_id: String,
    #[serde(default)]
    pub package: PackageInput,
    #[serde(default = "knowledge")]
    pub content_level: BundleContentLevel,
    #[serde(default = "all")]
    pub selector: Value,
    #[serde(default)]
    pub preview: bool,
    #[serde(default = "yes")]
    pub include_vectors: bool,
    #[serde(default = "yes")]
    pub include_images: bool,
    #[serde(default)]
    pub include_image_related: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportInput {
    pub material_id: MaterialId,
    #[serde(default = "operation_id")]
    pub operation_id: String,
    #[serde(default = "merge")]
    pub mode: BundleInstallMode,
    #[serde(default = "global")]
    pub scope_type: String,
    #[serde(default)]
    pub chat_id: String,
}

fn operation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn limit() -> usize {
    50
}
fn knowledge() -> BundleContentLevel {
    BundleContentLevel::Knowledge
}
fn merge() -> BundleInstallMode {
    BundleInstallMode::Merge
}
fn global() -> String {
    "global".into()
}
fn yes() -> bool {
    true
}
fn all() -> Value {
    serde_json::json!({"type":"all"})
}
