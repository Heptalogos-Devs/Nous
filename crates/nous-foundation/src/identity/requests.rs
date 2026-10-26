//! Identity management inputs contain policy and references, never credential bytes.
use nous_domain::{AccessGrant, AccessScope, GrantId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueGrant {
    pub name: String,
    pub scope: AccessScope,
    pub can_delegate: bool,
    pub expires_at_unix_ms: Option<i64>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GrantPage {
    pub grants: Vec<AccessGrant>,
    pub next: Option<GrantId>,
    pub has_more: bool,
}
