//! Bootstrap configuration definitions and restored credential binding.
use super::*;
use crate::{identity::AuthConfig, instance::ServerConfig, network::NetworkConfig};
use nous_domain::{ConfigApplyMode, ConfigEntry, ConfigLevel, ConfigScope, Result};

pub const SERVER: ConfigKey<ServerConfig> = ConfigKey::new("server");
pub const AUTH: ConfigKey<AuthConfig> = ConfigKey::new("auth");
pub const NETWORK: ConfigKey<NetworkConfig> = ConfigKey::new("network");

pub fn register_core(builder: &mut ConfigRegistryBuilder) -> Result<()> {
    builder.register(
        SERVER,
        ConfigDefinition {
            write_policy: crate::configuration::ConfigWritePolicy::Scoped,
            owner: "server".into(),
            description: "Instance listener and request body limit.".into(),
            default: ServerConfig::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Restart,
        },
        |value| {
            value
                .validate()
                .map_err(|error| nous_domain::Error::invalid(error.to_string()))
        },
    )?;
    builder.register(
        AUTH,
        ConfigDefinition {
            write_policy: crate::configuration::ConfigWritePolicy::LocalBootstrapOnly,
            owner: "identity".into(),
            description: "Bootstrap administrator seed; current API credentials belong to grants."
                .into(),
            default: AuthConfig::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Restart,
        },
        |value| {
            value
                .validate()
                .map_err(|error| nous_domain::Error::invalid(error.to_string()))
        },
    )?;
    builder.register(
        NETWORK,
        ConfigDefinition {
            write_policy: crate::configuration::ConfigWritePolicy::Scoped,
            owner: "network".into(),
            description: "Outbound proxy and connection/request timeouts.".into(),
            default: NetworkConfig::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Restart,
        },
        |value| {
            value
                .validate()
                .map_err(|error| nous_domain::Error::invalid(error.to_string()))
        },
    )
}

/// The local bootstrap owner installs a seed it already published and bound in this operation.
pub async fn install_bootstrap_seed_in(
    tx: &mut nous_storage::WriteTransaction,
    config: &AuthConfig,
) -> Result<ConfigEntry> {
    ConfigWritePolicy::LocalBootstrapOnly.require(tx.context())?;
    ConfigScope::Instance.require_access(tx.context())?;
    config.validate()?;
    let value = serde_json::to_value(config)?;
    let entry = ConfigEntry {
        scope: ConfigScope::Instance,
        key: AUTH.path().into(),
        revision: tx.commit_seq(),
        active: Some(value.clone()),
        desired: Some(value),
        apply_mode: ConfigApplyMode::Restart,
        recorded_at: tx.time().recorded(),
        commit_seq: tx.commit_seq(),
    };
    tx.insert_configuration_change(&entry).await?;
    Ok(entry)
}
