//! Acknowledgement of services created by the application host.
use crate::App;
use nous_domain::{AccessScope, CallContext, ConfigScope, Result};
use nous_foundation::{
    configuration::{AUTH, ConfigActivation, NETWORK, SERVER},
    instance::ServerConfig,
};
use nous_storage::BeginOperation;
use nous_storage::operation_key;
use serde_json::json;

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
        let key = operation_key(
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
