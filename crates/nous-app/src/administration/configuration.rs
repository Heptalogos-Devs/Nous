//! Authenticated configuration catalog, snapshots, atomic edits and immutable history.
use crate::App;
use nous_domain::{CallContext, ConfigEntry, ConfigScope, Result};
use nous_foundation::configuration::{
    ConfigChangeSet, ConfigDescriptor, ConfigEdit, ConfigurationView,
};
use nous_storage::operation_key;
use nous_storage::{BeginOperation, OperationReceipt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Validate the offline deployment through the same registered consumers as a running App.
pub fn deployment_snapshot(
    config: &nous_foundation::instance::InstanceConfig,
) -> Result<nous_foundation::configuration::ConfigSnapshot> {
    let roles = crate::composition::model_roles()?;
    crate::composition::configuration_registry(&roles)?
        .deployment_snapshot(&serde_json::to_value(config)?)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigChangeRequest {
    pub operation_id: String,
    pub scope: ConfigScope,
    pub edits: Vec<ConfigEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigRequest {
    Catalog {
        scope: Option<ConfigScope>,
    },
    Show {
        scope: Option<ConfigScope>,
    },
    Change(ConfigChangeRequest),
    History {
        scope: Option<ConfigScope>,
        key: Option<String>,
        #[serde(default = "history_limit")]
        limit: usize,
    },
}
fn history_limit() -> usize {
    50
}

impl App {
    pub async fn configuration_request(
        &self,
        ctx: &CallContext,
        request: ConfigRequest,
    ) -> Result<Value> {
        match request {
            ConfigRequest::Catalog { scope } => Ok(json!(
                self.config_catalog(ctx, &scope.unwrap_or_else(|| ConfigScope::for_caller(ctx)))
                    .await?
            )),
            ConfigRequest::Show { scope } => Ok(json!(
                self.config_view(ctx, &scope.unwrap_or_else(|| ConfigScope::for_caller(ctx)))
                    .await?
            )),
            ConfigRequest::History { scope, key, limit } => Ok(json!(
                self.config_history(
                    ctx,
                    &scope.unwrap_or_else(|| ConfigScope::for_caller(ctx)),
                    key.as_deref(),
                    limit
                )
                .await?
            )),
            ConfigRequest::Change(request) => Ok(json!(self.config_change(ctx, request).await?)),
        }
    }
    pub async fn config_catalog(
        &self,
        ctx: &CallContext,
        scope: &ConfigScope,
    ) -> Result<Vec<ConfigDescriptor>> {
        scope.require_access(ctx)?;
        let mut view = self.database.read_view().await?;
        view.validate_configuration_scope(scope).await?;
        view.finish().await?;
        Ok(self
            .configuration
            .registry()
            .descriptors()
            .cloned()
            .collect())
    }
    pub async fn config_view(
        &self,
        ctx: &CallContext,
        scope: &ConfigScope,
    ) -> Result<ConfigurationView> {
        scope.require_access(ctx)?;
        self.configuration.view(scope).await
    }
    pub async fn config_history(
        &self,
        ctx: &CallContext,
        scope: &ConfigScope,
        key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConfigEntry>> {
        scope.require_access(ctx)?;
        self.configuration.history(scope, key, limit).await
    }
    pub async fn config_change(
        &self,
        ctx: &CallContext,
        request: ConfigChangeRequest,
    ) -> Result<OperationReceipt> {
        request.scope.require_access(ctx)?;
        let key = operation_key(
            ctx,
            "configuration.change",
            &serde_json::to_string(&request.scope)?,
            &request.operation_id,
            &json!({"scope":request.scope,"edits":request.edits}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let change: ConfigChangeSet = self
            .configuration
            .change_in(&mut tx, &request.scope, &request.edits)
            .await?;
        let receipt = tx.commit(json!(change)).await?;
        self.configuration.committed(&change);
        self.tasks.notify_wake();
        Ok(receipt)
    }
}
