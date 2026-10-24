//! Core configuration definitions and acknowledgement by the running resource owner.
use super::*;
use crate::{
    App,
    common::{
        clock::Clock,
        instance::{AuthConfig, ServerConfig},
        network::NetworkConfig,
    },
};
use nous_domain::{
    AccessScope, CallContext, ConfigApplyMode, ConfigEntry, ConfigLevel, ConfigScope, Result,
};
use nous_storage::{BeginOperation, Database};
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

impl App {
    /// Called after listener binding and token loading succeed.
    pub async fn activate_bootstrap_configuration(&self, server: &ServerConfig) -> Result<()> {
        let values = [
            (SERVER.path(), serde_json::to_value(server)?),
            (
                AUTH.path(),
                serde_json::to_value(&self.bootstrap_config().auth)?,
            ),
            (
                NETWORK.path(),
                serde_json::to_value(&self.bootstrap_config().network)?,
            ),
        ];
        let body = json!(values);
        let ctx = CallContext {
            caller_id: "runtime".into(),
            scope: AccessScope::Admin,
        };
        let key = crate::operation_key(
            &ctx,
            "configuration.core.applied",
            "instance",
            &ulid::Ulid::generate().to_string(),
            &body,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(_) => return Ok(()),
            BeginOperation::New(tx) => tx,
        };
        let heads = tx.configuration_entries(&[ConfigScope::Instance]).await?;
        let applied = values
            .into_iter()
            .map(|(key, applied)| ConfigActivation {
                expected_revision: heads
                    .iter()
                    .find(|entry| entry.key == key)
                    .map_or(0, |entry| entry.revision),
                key: key.into(),
                applied,
            })
            .collect::<Vec<_>>();
        let change = self
            .configuration
            .activate_in(&mut tx, &ConfigScope::Instance, &applied)
            .await?;
        if change.changes.is_empty() {
            tx.rollback().await?;
            return Ok(());
        }
        tx.commit(json!(change)).await?;
        self.configuration.committed(&change);
        self.tasks.notify_wake();
        Ok(())
    }
}

/// Restore has installed a new administrator credential; preserve history and reset its binding.
pub async fn reset_restored_auth(database: &Database) -> Result<()> {
    let value = serde_json::to_value(AuthConfig::default())?;
    let ctx = CallContext {
        caller_id: "restore".into(),
        scope: AccessScope::Admin,
    };
    let key = crate::operation_key(
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
