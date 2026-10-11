// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{
    ConfigApplyMode, ConfigRegistry, ConfigScopePolicy, ConfigSnapshot, ConfigSource,
    ConfigStoragePolicy,
};
use chrono::Utc;
use nous_core::{DomainError, DomainErrorCode, Error, OperationId, Result, SubjectId};
use nous_persistence::{AuthorityStore, database_error as db};
use serde_json::Value;
use sqlx::Row;
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ConfigChangeOutcome {
    pub revision: i64,
    pub apply_mode: ConfigApplyMode,
    pub pending_restart: bool,
    pub active_digest: String,
    pub desired_digest: String,
}

#[derive(Clone)]
pub struct ConfigurationService {
    inner: Arc<Inner>,
}

struct Inner {
    store: AuthorityStore,
    registry: ConfigRegistry,
    deployment: BTreeMap<String, Value>,
    state: RwLock<ConfigurationState>,
    mutation: Mutex<()>,
}

#[derive(Clone)]
struct ConfigurationState {
    revision: i64,
    active_system: BTreeMap<String, Value>,
    desired_system: BTreeMap<String, Value>,
    active_subject: BTreeMap<SubjectId, BTreeMap<String, Value>>,
    desired_subject: BTreeMap<SubjectId, BTreeMap<String, Value>>,
}

impl ConfigurationService {
    pub async fn open(
        store: AuthorityStore,
        registry: ConfigRegistry,
        deployment: Value,
    ) -> Result<Self> {
        let deployment = registry.deployment_values(&deployment)?;
        let revision = sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM configuration_state WHERE singleton=TRUE",
        )
        .fetch_one(store.pool())
        .await
        .map_err(db)?;
        let system_rows =
            sqlx::query("SELECT key,value FROM system_configuration_overrides ORDER BY key")
                .fetch_all(store.pool())
                .await
                .map_err(db)?;
        let mut system = BTreeMap::new();
        for row in system_rows {
            let key: String = row.try_get("key").map_err(db)?;
            let value: Value = row.try_get("value").map_err(db)?;
            validate_persisted(&registry, &key, &value, false)?;
            system.insert(key, value);
        }
        let subject_rows = sqlx::query(
            "SELECT subject_id,key,value FROM subject_configuration_overrides ORDER BY subject_id,key",
        )
        .fetch_all(store.pool())
        .await
        .map_err(db)?;
        let mut subjects = BTreeMap::<SubjectId, BTreeMap<String, Value>>::new();
        for row in subject_rows {
            let subject = SubjectId(row.try_get("subject_id").map_err(db)?);
            let key: String = row.try_get("key").map_err(db)?;
            let value: Value = row.try_get("value").map_err(db)?;
            validate_persisted(&registry, &key, &value, true)?;
            subjects.entry(subject).or_default().insert(key, value);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                registry,
                deployment,
                state: RwLock::new(ConfigurationState {
                    revision,
                    active_system: system.clone(),
                    desired_system: system,
                    active_subject: subjects.clone(),
                    desired_subject: subjects,
                }),
                mutation: Mutex::const_new(()),
            }),
        })
    }

    pub fn registry(&self) -> &ConfigRegistry {
        &self.inner.registry
    }

    pub fn active_system_snapshot(&self) -> Result<ConfigSnapshot> {
        self.snapshot(None, false)
    }

    pub fn desired_system_snapshot(&self) -> Result<ConfigSnapshot> {
        self.snapshot(None, true)
    }

    pub fn snapshot_for_subject(&self, subject: SubjectId) -> Result<ConfigSnapshot> {
        self.snapshot(Some(subject), false)
    }

    pub fn desired_snapshot_for_subject(&self, subject: SubjectId) -> Result<ConfigSnapshot> {
        self.snapshot(Some(subject), true)
    }

    fn snapshot(&self, subject: Option<SubjectId>, desired: bool) -> Result<ConfigSnapshot> {
        let state = self
            .inner
            .state
            .read()
            .map_err(|_| Error::Internal("configuration state lock poisoned".into()))?;
        self.snapshot_from_state(&state, subject, desired)
    }

    fn snapshot_from_state(
        &self,
        state: &ConfigurationState,
        subject: Option<SubjectId>,
        desired: bool,
    ) -> Result<ConfigSnapshot> {
        let system = if desired {
            &state.desired_system
        } else {
            &state.active_system
        };
        let subject_values = subject.and_then(|id| {
            if desired {
                state.desired_subject.get(&id)
            } else {
                state.active_subject.get(&id)
            }
        });
        let mut values = BTreeMap::new();
        for descriptor in self.inner.registry.descriptors() {
            let (value, source) = if let Some(value) = subject_values
                .and_then(|values| values.get(&descriptor.path))
                .filter(|_| descriptor.scope_policy == ConfigScopePolicy::SubjectOverrideAllowed)
            {
                (value.clone(), ConfigSource::PersistedSubject)
            } else if let Some(value) = system.get(&descriptor.path) {
                (value.clone(), ConfigSource::PersistedSystem)
            } else if let Some(value) = self.inner.deployment.get(&descriptor.path) {
                (value.clone(), ConfigSource::DeploymentFile)
            } else {
                (
                    descriptor.reference_default.clone(),
                    ConfigSource::ReferenceDefault,
                )
            };
            values.insert(
                descriptor.path.clone(),
                crate::ResolvedConfigValue {
                    json: value,
                    source,
                },
            );
        }
        ConfigSnapshot::new(subject, state.revision, self.inner.registry.clone(), values)
    }

    fn scoped_digests(
        &self,
        state: &ConfigurationState,
        subject: Option<SubjectId>,
    ) -> Result<(String, String)> {
        let active = self
            .snapshot_from_state(state, subject, false)?
            .effective_digest
            .clone();
        let desired = self
            .snapshot_from_state(state, subject, true)?
            .effective_digest
            .clone();
        Ok((active, desired))
    }

    pub async fn set_system_override(
        &self,
        operation_id: OperationId,
        key: &str,
        value: Value,
        expected_revision: Option<i64>,
    ) -> Result<ConfigChangeOutcome> {
        self.mutate(operation_id, None, key, Some(value), expected_revision)
            .await
    }

    pub async fn clear_system_override(
        &self,
        operation_id: OperationId,
        key: &str,
        expected_revision: Option<i64>,
    ) -> Result<ConfigChangeOutcome> {
        self.mutate(operation_id, None, key, None, expected_revision)
            .await
    }

    pub async fn set_subject_override(
        &self,
        operation_id: OperationId,
        subject: SubjectId,
        key: &str,
        value: Value,
        expected_revision: Option<i64>,
    ) -> Result<ConfigChangeOutcome> {
        self.mutate(
            operation_id,
            Some(subject),
            key,
            Some(value),
            expected_revision,
        )
        .await
    }

    pub async fn clear_subject_override(
        &self,
        operation_id: OperationId,
        subject: SubjectId,
        key: &str,
        expected_revision: Option<i64>,
    ) -> Result<ConfigChangeOutcome> {
        self.mutate(operation_id, Some(subject), key, None, expected_revision)
            .await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "configuration mutation keeps authorization, receipt, persistence, and cache update in one transaction contract"
    )]
    async fn mutate(
        &self,
        operation_id: OperationId,
        subject: Option<SubjectId>,
        key: &str,
        value: Option<Value>,
        expected_revision: Option<i64>,
    ) -> Result<ConfigChangeOutcome> {
        let _mutation = self.inner.mutation.lock().await;
        if expected_revision.is_some_and(|revision| revision < 0) {
            return Err(Error::Invalid(
                "expected configuration revision must be nonnegative".into(),
            ));
        }
        if operation_id.0.is_nil() {
            return Err(Error::Invalid(
                "configuration operation_id is required".into(),
            ));
        }
        let descriptor = self
            .inner
            .registry
            .descriptor(key)
            .ok_or_else(|| Error::Invalid(format!("unknown configuration key: {key}")))?;
        if descriptor.storage_policy == ConfigStoragePolicy::DeploymentOnly {
            return Err(Error::Invalid(format!(
                "configuration path is deployment-only: {key}"
            )));
        }
        let value = value
            .map(|value| self.inner.registry.normalize(key, &value))
            .transpose()?;
        if subject.is_some() && descriptor.scope_policy != ConfigScopePolicy::SubjectOverrideAllowed
        {
            return Err(Error::Invalid(format!(
                "configuration key is system-only: {key}"
            )));
        }
        let action = if value.is_some() { "set" } else { "clear" };
        let request = serde_json::json!({
            "subject_id": subject.map(|value| value.0),
            "key": key,
            "value": value,
            "action": action,
            "expected_revision": expected_revision,
        });
        let digest = blake3::hash(
            serde_json::to_string(&request)
                .map_err(|error| Error::Internal(error.to_string()))?
                .as_bytes(),
        )
        .to_hex()
        .to_string();
        let mut next_state = self
            .inner
            .state
            .read()
            .map_err(|_| Error::Internal("configuration state lock poisoned".into()))?
            .clone();
        let mut tx = self.inner.store.begin().await?;
        let existing = sqlx::query(
            "SELECT request_digest,revision,apply_mode,pending_restart,active_digest,desired_digest FROM configuration_mutation_receipts WHERE operation_id=$1 FOR UPDATE",
        )
        .bind(operation_id.0)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if let Some(row) = existing {
            let existing_digest: String = row.try_get("request_digest").map_err(db)?;
            if existing_digest != digest {
                return Err(DomainError::new(
                    DomainErrorCode::OperationIdConflict,
                    "Configuration operation_id was used with a different request",
                )
                .with_context("operation_id", operation_id.0)
                .with_context("path", key)
                .into());
            }
            let outcome = ConfigChangeOutcome {
                revision: row.try_get("revision").map_err(db)?,
                apply_mode: parse_apply_mode(row.try_get("apply_mode").map_err(db)?)?,
                pending_restart: row.try_get("pending_restart").map_err(db)?,
                active_digest: row.try_get("active_digest").map_err(db)?,
                desired_digest: row.try_get("desired_digest").map_err(db)?,
            };
            tx.commit().await.map_err(db)?;
            return Ok(outcome);
        }
        let revision: i64 = sqlx::query_scalar(
            "UPDATE configuration_state SET revision=revision+1 WHERE singleton=TRUE AND ($1::bigint IS NULL OR revision=$1) RETURNING revision",
        )
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| DomainError::new(DomainErrorCode::StaleRevision, "Configuration revision changed; read the current desired policy before replacing it")
            .with_context("path", key))?;
        match (subject, value.as_ref()) {
            (Some(subject), Some(value)) => {
                sqlx::query("INSERT INTO subject_configuration_overrides(subject_id,key,value,revision,updated_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(subject_id,key) DO UPDATE SET value=excluded.value,revision=excluded.revision,updated_at=excluded.updated_at")
                    .bind(subject.0).bind(key).bind(value).bind(revision).bind(Utc::now()).execute(&mut *tx).await.map_err(db)?;
            }
            (Some(subject), None) => {
                sqlx::query(
                    "DELETE FROM subject_configuration_overrides WHERE subject_id=$1 AND key=$2",
                )
                .bind(subject.0)
                .bind(key)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
            (None, Some(value)) => {
                sqlx::query("INSERT INTO system_configuration_overrides(key,value,revision,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(key) DO UPDATE SET value=excluded.value,revision=excluded.revision,updated_at=excluded.updated_at")
                    .bind(key).bind(value).bind(revision).bind(Utc::now()).execute(&mut *tx).await.map_err(db)?;
            }
            (None, None) => {
                sqlx::query("DELETE FROM system_configuration_overrides WHERE key=$1")
                    .bind(key)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
            }
        }
        let pending_restart = descriptor.apply_mode == ConfigApplyMode::RestartProcess;
        apply_mutation_to_state(
            &mut next_state,
            subject,
            key,
            value.clone(),
            pending_restart,
        );
        next_state.revision = revision;
        let (active_digest, desired_digest) = self.scoped_digests(&next_state, subject)?;
        sqlx::query("INSERT INTO configuration_mutation_receipts(operation_id,request_digest,subject_id,key,action,revision,apply_mode,pending_restart,active_digest,desired_digest,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(operation_id.0).bind(&digest).bind(subject.map(|value| value.0)).bind(key).bind(action).bind(revision).bind(format_apply_mode(descriptor.apply_mode)).bind(pending_restart).bind(&active_digest).bind(&desired_digest).bind(Utc::now()).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        {
            let mut state = self
                .inner
                .state
                .write()
                .map_err(|_| Error::Internal("configuration state lock poisoned".into()))?;
            *state = next_state;
        }
        Ok(ConfigChangeOutcome {
            revision,
            apply_mode: descriptor.apply_mode,
            pending_restart,
            active_digest,
            desired_digest,
        })
    }
}

fn apply_mutation_to_state(
    state: &mut ConfigurationState,
    subject: Option<SubjectId>,
    key: &str,
    value: Option<Value>,
    pending_restart: bool,
) {
    let desired = if let Some(subject) = subject {
        state.desired_subject.entry(subject).or_default()
    } else {
        &mut state.desired_system
    };
    apply_map(desired, key, value.clone());
    if !pending_restart {
        let active = if let Some(subject) = subject {
            state.active_subject.entry(subject).or_default()
        } else {
            &mut state.active_system
        };
        apply_map(active, key, value);
    }
}

fn validate_persisted(
    registry: &ConfigRegistry,
    key: &str,
    value: &Value,
    subject: bool,
) -> Result<()> {
    let descriptor = registry.descriptor(key).ok_or_else(|| {
        Error::Infrastructure(format!("persisted unknown configuration key: {key}"))
    })?;
    if subject && descriptor.scope_policy != ConfigScopePolicy::SubjectOverrideAllowed {
        return Err(Error::Infrastructure(format!(
            "persisted subject override is system-only: {key}"
        )));
    }
    if descriptor.storage_policy == ConfigStoragePolicy::DeploymentOnly {
        return Err(Error::Infrastructure(format!(
            "persisted deployment-only override: {key}"
        )));
    }
    registry.validate(key, value)
}

fn apply_map(map: &mut BTreeMap<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), value);
    } else {
        map.remove(key);
    }
}

fn format_apply_mode(value: ConfigApplyMode) -> &'static str {
    match value {
        ConfigApplyMode::Live => "live",
        ConfigApplyMode::ServingRebuild => "serving_rebuild",
        ConfigApplyMode::NewSubjectsOnly => "new_subjects_only",
        ConfigApplyMode::RestartProcess => "restart_process",
    }
}

fn parse_apply_mode(value: String) -> Result<ConfigApplyMode> {
    match value.as_str() {
        "live" => Ok(ConfigApplyMode::Live),
        "serving_rebuild" => Ok(ConfigApplyMode::ServingRebuild),
        "new_subjects_only" => Ok(ConfigApplyMode::NewSubjectsOnly),
        "restart_process" => Ok(ConfigApplyMode::RestartProcess),
        _ => Err(Error::Infrastructure(
            "invalid configuration apply mode".into(),
        )),
    }
}
