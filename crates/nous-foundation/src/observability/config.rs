use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

pub const OBSERVABILITY: ConfigKey<ObservabilityConfig> = ConfigKey::new("observability");

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ObservabilityConfig {
    pub filter: String,
    pub stderr: bool,
    pub file: bool,
    pub retained_files: usize,
    pub buffered_lines: usize,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            filter: "info,sqlx=warn,tantivy=warn,hyper=warn,reqwest=warn".into(),
            stderr: true,
            file: true,
            retained_files: 14,
            buffered_lines: 8192,
        }
    }
}

impl ObservabilityConfig {
    pub fn validate(&self) -> Result<()> {
        EnvFilter::try_new(&self.filter)
            .map_err(|error| Error::invalid(format!("invalid observability.filter: {error}")))?;
        if self.retained_files == 0 || self.retained_files > 3650 {
            return Err(Error::invalid(
                "observability.retained_files must be 1 to 3650",
            ));
        }
        if self.buffered_lines == 0 || self.buffered_lines > 1_048_576 {
            return Err(Error::invalid(
                "observability.buffered_lines must be 1 to 1048576",
            ));
        }
        Ok(())
    }
}

pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        OBSERVABILITY,
        ConfigDefinition {
            write_policy: crate::configuration::ConfigWritePolicy::Scoped,
            owner: "observability".into(),
            description: "Process-wide trace filtering, JSON log outputs and daily log retention"
                .into(),
            default: ObservabilityConfig::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Restart,
        },
        ObservabilityConfig::validate,
    )
}
