//! Authority-backed configuration snapshots and committed change notifications.
use super::{ConfigRegistry, ConfigSnapshot};
use crate::clock::Clock;
use nous_domain::{ConfigEntry, ConfigScope, Result};
use nous_storage::{Database, ReadView, WriteTransaction};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::watch;

#[derive(Debug, Clone, Serialize)]
pub struct ConfigChange {
    pub entry: ConfigEntry,
    pub pending_restart: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigChangeSet {
    pub scope: ConfigScope,
    pub revision: i64,
    pub changes: Vec<ConfigChange>,
    pub active_digest: String,
    pub desired_digest: String,
    pub pending_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigurationView {
    pub active: ConfigSnapshot,
    pub desired: ConfigSnapshot,
    pub overrides: Vec<ConfigEntry>,
    pub pending_keys: Vec<String>,
}

#[derive(Clone)]
pub struct Configuration {
    pub(super) database: Database,
    pub(super) registry: ConfigRegistry,
    pub(super) deployment: Arc<BTreeMap<String, Value>>,
    changes: watch::Sender<i64>,
    pub(super) clock: Clock,
}

impl Configuration {
    pub async fn new(
        database: Database,
        registry: ConfigRegistry,
        deployment_document: &Value,
        clock: Clock,
    ) -> Result<Self> {
        let deployment = registry.deployment_values(deployment_document)?;
        let mut view = database.read_view().await?;
        let revision = view.configuration_revision().await?;
        let (changes, _) = watch::channel(revision);
        let service = Self {
            database: database.clone(),
            registry,
            deployment: Arc::new(deployment),
            changes,
            clock,
        };
        for scope in view.configuration_scopes().await? {
            let entries = view
                .configuration_entries(&super::resolution::hierarchy(&scope))
                .await?;
            service.resolve(&scope, revision, &entries, false)?;
            service.resolve(&scope, revision, &entries, true)?;
        }
        view.finish().await?;
        Ok(service)
    }
    pub fn registry(&self) -> &ConfigRegistry {
        &self.registry
    }
    pub fn watch(&self) -> watch::Receiver<i64> {
        self.changes.subscribe()
    }

    /// Publish only after the containing Authority Operation commits.
    pub fn committed(&self, change: &ConfigChangeSet) {
        self.changes.send_if_modified(|revision| {
            if change.revision <= *revision {
                return false;
            }
            *revision = change.revision;
            true
        });
    }
    pub async fn snapshot(&self, scope: &ConfigScope, desired: bool) -> Result<ConfigSnapshot> {
        let mut view = self.database.read_view().await?;
        let snapshot = self.snapshot_in(&mut view, scope, desired).await?;
        view.finish().await?;
        Ok(snapshot)
    }
    pub async fn snapshot_in(
        &self,
        view: &mut ReadView,
        scope: &ConfigScope,
        desired: bool,
    ) -> Result<ConfigSnapshot> {
        view.validate_configuration_scope(scope).await?;
        let entries = view
            .configuration_entries(&super::resolution::hierarchy(scope))
            .await?;
        self.resolve(
            scope,
            view.configuration_revision().await?,
            &entries,
            desired,
        )
    }
    pub async fn snapshot_in_transaction(
        &self,
        tx: &mut WriteTransaction,
        scope: &ConfigScope,
        desired: bool,
    ) -> Result<ConfigSnapshot> {
        tx.validate_configuration_scope(scope).await?;
        let entries = tx
            .configuration_entries(&super::resolution::hierarchy(scope))
            .await?;
        self.resolve(scope, tx.configuration_revision().await?, &entries, desired)
    }
    pub async fn view(&self, scope: &ConfigScope) -> Result<ConfigurationView> {
        let mut view = self.database.read_view().await?;
        view.validate_configuration_scope(scope).await?;
        let overrides = view
            .configuration_entries(&super::resolution::hierarchy(scope))
            .await?;
        let revision = view.configuration_revision().await?;
        let active = self.resolve(scope, revision, &overrides, false)?;
        let desired = self.resolve(scope, revision, &overrides, true)?;
        let pending_keys = super::resolution::pending_keys(&active, &desired);
        view.finish().await?;
        Ok(ConfigurationView {
            active,
            desired,
            overrides,
            pending_keys,
        })
    }
    pub async fn history(
        &self,
        scope: &ConfigScope,
        key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConfigEntry>> {
        if !(1..=1000).contains(&limit) {
            return Err(nous_domain::Error::invalid(
                "configuration history limit must be 1..1000",
            ));
        }
        if let Some(key) = key {
            self.registry.descriptor(key)?;
        }
        let mut view = self.database.read_view().await?;
        view.validate_configuration_scope(scope).await?;
        view.finish().await?;
        self.database.configuration_history(scope, key, limit).await
    }
}
