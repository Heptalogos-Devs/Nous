//! Authenticated configuration catalog, snapshots, atomic edits and immutable history.
use crate::App;
use nous_domain::{CallContext, ConfigEntry, ConfigScope, Result};
use nous_foundation::configuration::{ConfigDescriptor, ConfigEdit, ConfigurationView};
use nous_storage::OperationReceipt;
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
    pub async fn change_offline_configuration(
        paths: &nous_foundation::instance::InstancePaths,
        request: ConfigChangeRequest,
    ) -> Result<OperationReceipt> {
        use nous_foundation::{
            clock::Clock,
            configuration::Configuration,
            instance::{InstanceConfig, InstanceGuard},
        };
        let _guard = InstanceGuard::acquire(paths).map_err(|error| {
            nous_domain::Error::new(
                nous_domain::ErrorCode::DependencyUnavailable,
                error.to_string(),
            )
            .with_cause(error.as_ref())
        })?;
        let deployment = InstanceConfig::load(paths).map_err(|error| {
            nous_domain::Error::invalid(error.to_string()).with_cause(error.as_ref())
        })?;
        let roles = crate::composition::model_roles()?;
        let registry = crate::composition::configuration_registry(&roles)?;
        let database =
            nous_storage::Database::open(&paths.database, Clock::operation_snapshot).await?;
        let result = async {
            let configuration = Configuration::new(
                database.clone(),
                registry,
                &serde_json::to_value(deployment)?,
            )
            .await?;
            configuration
                .apply(
                    &CallContext::admin(),
                    &request.operation_id,
                    &request.scope,
                    &request.edits,
                )
                .await
        }
        .await;
        database.close().await;
        result
    }

    pub async fn desired_instance_configuration(
        paths: &nous_foundation::instance::InstancePaths,
    ) -> Result<nous_foundation::instance::InstanceConfig> {
        use nous_foundation::{
            clock::Clock, configuration::Configuration, instance::InstanceConfig,
        };
        let deployment = if paths.config.exists() {
            InstanceConfig::load(paths)
                .map_err(|error| nous_domain::Error::invalid(error.to_string()))?
        } else {
            InstanceConfig::default()
        };
        let roles = crate::composition::model_roles()?;
        let registry = crate::composition::configuration_registry(&roles)?;
        let document = serde_json::to_value(deployment)?;
        if !paths.database.exists() {
            return InstanceConfig::from_snapshot(&registry.deployment_snapshot(&document)?);
        }
        let database =
            nous_storage::Database::open_read_only(&paths.database, Clock::operation_snapshot)
                .await?;
        let result = async {
            let configuration = Configuration::new(database.clone(), registry, &document).await?;
            InstanceConfig::from_snapshot(
                &configuration.snapshot(&ConfigScope::Instance, true).await?,
            )
        }
        .await;
        database.close().await;
        result
    }

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
        let receipt = self
            .configuration
            .apply(ctx, &request.operation_id, &request.scope, &request.edits)
            .await?;
        self.tasks.notify_wake();
        Ok(receipt)
    }
}
