//! The bootstrap credential seeds one administrator grant before online authorization exists.
use crate::secret::SecretRef;
use nous_domain::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    pub admin_token: SecretRef,
}
impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            admin_token: SecretRef {
                file: PathBuf::from("credentials/admin.token"),
            },
        }
    }
}
impl AuthConfig {
    pub fn validate(&self) -> Result<()> {
        self.admin_token.validate()
    }
}
