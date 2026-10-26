//! Runtime scheduling policy shared by submission and worker execution.
use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TASK_RUNTIME: ConfigKey<TaskRuntimeConfig> = ConfigKey::new("tasks");

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TaskRuntimeConfig {
    pub concurrency: usize,
    pub poll_interval_ms: u64,
    pub wake_interval_ms: u64,
    pub max_queue_attempts: u32,
}

impl Default for TaskRuntimeConfig {
    fn default() -> Self {
        Self {
            concurrency: 1,
            poll_interval_ms: 250,
            wake_interval_ms: 30_000,
            max_queue_attempts: 5,
        }
    }
}

impl TaskRuntimeConfig {
    pub fn validate(&self) -> Result<()> {
        if !(1..=256).contains(&self.concurrency)
            || !(1..=86_400_000).contains(&self.poll_interval_ms)
            || !(1..=86_400_000).contains(&self.wake_interval_ms)
            || !(1..=1000).contains(&self.max_queue_attempts)
        {
            return Err(Error::invalid(
                "tasks require concurrency 1..256, intervals 1..86400000 ms and max_queue_attempts 1..1000",
            ));
        }
        Ok(())
    }
}

pub fn register_configuration(builder: &mut ConfigRegistryBuilder) -> Result<()> {
    builder.register(
        TASK_RUNTIME,
        ConfigDefinition {
            write_policy: crate::configuration::ConfigWritePolicy::Scoped,
            owner: "tasks".into(),
            description:
                "Worker concurrency, queue polling, waiting checks and queue retry budget.".into(),
            default: TaskRuntimeConfig::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Restart,
        },
        TaskRuntimeConfig::validate,
    )
}
