use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, PathBuf},
};

pub const PLATFORM: ConfigKey<PlatformConfig> = ConfigKey::new("platform");

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutableResource {
    Managed { file: PathBuf },
    External { path: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformConfig {
    pub executables: BTreeMap<String, ExecutableResource>,
    pub max_parallel: usize,
    pub max_runtime_ms: u64,
    pub stop_grace_ms: u64,
    pub cleanup_timeout_ms: u64,
    pub max_output_bytes: usize,
    pub max_stdin_bytes: usize,
    pub max_argument_bytes: usize,
}
impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            executables: BTreeMap::new(),
            max_parallel: 4,
            max_runtime_ms: 300_000,
            stop_grace_ms: 5_000,
            cleanup_timeout_ms: 10_000,
            max_output_bytes: 8 * 1024 * 1024,
            max_stdin_bytes: 1024 * 1024,
            max_argument_bytes: 1024 * 1024,
        }
    }
}
impl PlatformConfig {
    pub fn validate(&self) -> Result<()> {
        if self.executables.len() > 128
            || !(1..=256).contains(&self.max_parallel)
            || !(1..=86_400_000).contains(&self.max_runtime_ms)
            || self.stop_grace_ms > 60_000
            || !(1..=60_000).contains(&self.cleanup_timeout_ms)
            || !(1..=64 * 1024 * 1024).contains(&self.max_output_bytes)
            || self.max_stdin_bytes > 64 * 1024 * 1024
            || !(1..=16 * 1024 * 1024).contains(&self.max_argument_bytes)
        {
            return Err(Error::invalid("platform process limits are out of range"));
        }
        for (key, resource) in &self.executables {
            validate_name(key, "executable resource")?;
            match resource {
                ExecutableResource::Managed { file }
                    if file.as_os_str().is_empty()
                        || file.is_absolute()
                        || file
                            .components()
                            .any(|part| !matches!(part, Component::Normal(_))) =>
                {
                    return Err(Error::invalid(
                        "managed executable file must be a relative path beneath instance bin",
                    ));
                }
                ExecutableResource::External { path } if !path.is_absolute() => {
                    return Err(Error::invalid(
                        "external executable must have an absolute path",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        PLATFORM,
        ConfigDefinition {
            write_policy: crate::configuration::ConfigWritePolicy::Scoped,
            owner: "platform".into(),
            description: "Registered executables and managed process resource limits.".into(),
            default: PlatformConfig::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Restart,
        },
        PlatformConfig::validate,
    )
}
