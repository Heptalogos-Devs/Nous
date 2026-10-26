use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SERVICE: ConfigKey<ServicePolicy> = ConfigKey::new("service");
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ServicePolicy {
    pub powershell_resource: String,
    pub systemctl_resource: String,
    pub launchctl_resource: String,
    pub winsw_resource: String,
    pub id_resource: String,
    pub operation_timeout_ms: u64,
    pub stop_timeout_ms: u64,
    pub restart_delay_ms: u64,
    pub status_poll_ms: u64,
    pub windows_task_restart_count: u32,
}
impl Default for ServicePolicy {
    fn default() -> Self {
        Self {
            powershell_resource: "powershell".into(),
            systemctl_resource: "systemctl".into(),
            launchctl_resource: "launchctl".into(),
            winsw_resource: "winsw".into(),
            id_resource: "id".into(),
            operation_timeout_ms: 60_000,
            stop_timeout_ms: 30_000,
            restart_delay_ms: 60_000,
            status_poll_ms: 100,
            windows_task_restart_count: 3,
        }
    }
}
impl ServicePolicy {
    pub fn validate(&self) -> Result<()> {
        for key in [
            &self.powershell_resource,
            &self.systemctl_resource,
            &self.launchctl_resource,
            &self.winsw_resource,
            &self.id_resource,
        ] {
            validate_name(key, "service executable resource")?;
        }
        if self.operation_timeout_ms == 0
            || self.stop_timeout_ms == 0
            || self.restart_delay_ms == 0
            || self.status_poll_ms == 0
            || self.operation_timeout_ms > 3_600_000
            || self.stop_timeout_ms > self.operation_timeout_ms
            || self.status_poll_ms > self.operation_timeout_ms
            || self.restart_delay_ms > 86_400_000
            || !(1..=999).contains(&self.windows_task_restart_count)
        {
            return Err(Error::invalid("service lifecycle limits are out of range"));
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        SERVICE,
        ConfigDefinition {
            owner: "service".into(),
            description: "Native user/system service manager resources and lifecycle limits."
                .into(),
            default: ServicePolicy::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Live,
            write_policy: ConfigWritePolicy::LocalBootstrapOnly,
        },
        ServicePolicy::validate,
    )
}
