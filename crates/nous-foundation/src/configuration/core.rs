//! Bootstrap configuration definitions and restored credential binding.
use super::*;
use crate::{
    clock::Clock,
    instance::{AuthConfig, ServerConfig},
    network::NetworkConfig,
};
use nous_domain::{
    AccessScope, CallContext, ConfigApplyMode, ConfigEntry, ConfigLevel, ConfigScope, Result,
};
use nous_storage::BeginOperation;
use nous_storage::Database;
use nous_storage::operation_key;
use serde_json::json;

pub const SERVER: ConfigKey<ServerConfig> = ConfigKey::new("server");
pub const AUTH: ConfigKey<AuthConfig> = ConfigKey::new("auth");
pub const NETWORK: ConfigKey<NetworkConfig> = ConfigKey::new("network");

pub fn register_core(builder: &mut ConfigRegistryBuilder) -> Result<()> {
    builder.register(
        SERVER,
        ConfigDefinition {
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
            owner: "identity".into(),
            description: "Protected administrator credential reference.".into(),
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

/// Restore has installed a new administrator credential; preserve history and reset its binding.
pub async fn reset_restored_auth(database: &Database) -> Result<()> {
    let value = serde_json::to_value(AuthConfig::default())?;
    let ctx = CallContext::new("restore", AccessScope::Admin);
    let key = operation_key(
        &ctx,
        "configuration.restore_auth",
        "instance",
        &ulid::Ulid::generate().to_string(),
        &value,
    )?;
    let mut tx = match database.begin_operation(key).await? {
        BeginOperation::Replay(_) => return Ok(()),
        BeginOperation::New(tx) => tx,
    };
    let entry = ConfigEntry {
        scope: ConfigScope::Instance,
        key: AUTH.path().into(),
        revision: tx.commit_seq(),
        active: Some(value.clone()),
        desired: Some(value),
        apply_mode: ConfigApplyMode::Restart,
        recorded_at: Clock.recorded(),
        commit_seq: tx.commit_seq(),
    };
    tx.insert_configuration_change(&entry).await?;
    tx.commit(
        json!({"key":AUTH.path(),"revision":entry.revision,"administrator_binding_reset":true}),
    )
    .await?;
    Ok(())
}
