use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const INSTALLATION: ConfigKey<InstallationPolicy> = ConfigKey::new("installation");

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct InstallationPolicy {
    pub max_executable_bytes: u64,
    pub download_timeout_ms: u64,
}
impl Default for InstallationPolicy {
    fn default() -> Self {
        Self {
            max_executable_bytes: 512 * 1024 * 1024,
            download_timeout_ms: 300_000,
        }
    }
}
impl InstallationPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.max_executable_bytes == 0 || self.download_timeout_ms == 0 {
            return Err(Error::invalid(
                "installation byte and download time limits must be positive",
            ));
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        INSTALLATION,
        ConfigDefinition {
            owner: "installation".into(),
            description: "Software artifact download and staging limits.".into(),
            default: InstallationPolicy::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Live,
            write_policy: ConfigWritePolicy::LocalBootstrapOnly,
        },
        InstallationPolicy::validate,
    )
}
