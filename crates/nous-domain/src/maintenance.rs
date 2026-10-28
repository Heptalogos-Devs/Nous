//! Memory lifecycle is separate from source existence and soft deletion.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOrigin {
    #[default]
    Model,
    Imported,
    Manual,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    #[default]
    Contact,
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryActivation {
    #[default]
    Active,
    Frozen,
    Forgotten,
    Weakened,
    Decayed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryLifecycle {
    pub strength: f64,
    pub anchor_unix_ms: i64,
    pub activation: MemoryActivation,
    pub pinned: bool,
    pub protected_until_unix_ms: Option<i64>,
    pub inactive_since_unix_ms: Option<i64>,
    pub last_reinforced_unix_ms: Option<i64>,
}
impl Default for MemoryLifecycle {
    fn default() -> Self {
        Self::new(0)
    }
}
impl MemoryLifecycle {
    pub fn new(anchor_unix_ms: i64) -> Self {
        Self {
            strength: 1.0,
            anchor_unix_ms,
            activation: MemoryActivation::Active,
            pinned: false,
            protected_until_unix_ms: None,
            inactive_since_unix_ms: None,
            last_reinforced_unix_ms: None,
        }
    }
    pub fn protected(&self, now_unix_ms: i64) -> bool {
        self.pinned
            || self
                .protected_until_unix_ms
                .is_some_and(|until| until > now_unix_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAction {
    Reinforce,
    Weaken,
    Protect,
    Unprotect,
    Freeze,
    Forget,
    Restore,
    Trash,
    Purge,
    Revise,
    Decay,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEdit {
    pub title: Option<String>,
    pub content: Option<String>,
    pub epistemic_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryChange {
    pub action: MemoryAction,
    pub expected_revision: i64,
    pub strength: Option<f64>,
    pub hours: Option<f64>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub force: bool,
    pub edit: Option<MemoryEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAudit {
    pub id: String,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub memory_id: MemoryId,
    pub action: String,
    pub actor: String,
    pub reason: String,
    pub previous_revision: i64,
    pub revision: i64,
    pub recorded_at: String,
    pub commit_seq: i64,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MaterialDetails {
    pub source_type: Option<String>,
    pub tags: Vec<String>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
    pub structured: Option<Value>,
}
impl Default for MaterialDetails {
    fn default() -> Self {
        Self {
            source_type: None,
            tags: vec![],
            metadata: empty_object(),
            structured: None,
        }
    }
}
pub fn empty_object() -> Value {
    serde_json::json!({})
}
