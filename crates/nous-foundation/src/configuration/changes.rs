//! Scope/key CAS batches validated as complete candidate snapshots before persistence.
use super::{ConfigChange, ConfigChangeSet, Configuration};
use nous_domain::{ConfigApplyMode, ConfigEntry, ConfigScope, Error, ErrorCode, Result};
use nous_storage::WriteTransaction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigEdit {
    Set {
        key: String,
        expected_revision: i64,
        value: Value,
    },
    Clear {
        key: String,
        expected_revision: i64,
    },
}
impl ConfigEdit {
    pub fn key(&self) -> &str {
        match self {
            Self::Set { key, .. } | Self::Clear { key, .. } => key,
        }
    }
    fn expected_revision(&self) -> i64 {
        match self {
            Self::Set {
                expected_revision, ..
            }
            | Self::Clear {
                expected_revision, ..
            } => *expected_revision,
        }
    }
}

pub struct ConfigActivation {
    pub key: String,
    pub expected_revision: i64,
    pub applied: Value,
}

impl Configuration {
    pub async fn change_in(
        &self,
        tx: &mut WriteTransaction,
        scope: &ConfigScope,
        edits: &[ConfigEdit],
    ) -> Result<ConfigChangeSet> {
        scope.require_access(tx.context())?;
        tx.validate_configuration_scope(scope).await?;
        if edits.is_empty() || edits.len() > 256 {
            return Err(Error::invalid("configuration batch requires 1..256 edits"));
        }
        let heads = tx
            .configuration_entries(std::slice::from_ref(scope))
            .await?;
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::with_capacity(edits.len());
        let recorded_at = tx.time().recorded();
        for edit in edits {
            let key = edit.key();
            if !seen.insert(key) {
                return Err(Error::invalid("configuration batch repeats a key"));
            }
            let definition = self.registry.descriptor(key)?;
            definition.write_policy.require(tx.context())?;
            if !definition.scopes.contains(&scope.level()) {
                return Err(Error::invalid(
                    "configuration key does not permit this scope",
                ));
            }
            let previous = heads.iter().find(|entry| entry.key == key);
            compare_revision(previous, edit.expected_revision())?;
            let desired = match edit {
                ConfigEdit::Set { value, .. } => Some(self.registry.normalize(key, value.clone())?),
                ConfigEdit::Clear { .. } => None,
            };
            let active = if definition.apply_mode == ConfigApplyMode::Live {
                desired.clone()
            } else {
                previous.and_then(|entry| entry.active.clone())
            };
            candidates.push(ConfigEntry {
                scope: scope.clone(),
                key: key.into(),
                revision: tx.commit_seq(),
                active,
                desired,
                apply_mode: definition.apply_mode,
                recorded_at: recorded_at.clone(),
                commit_seq: tx.commit_seq(),
            });
        }
        self.validate_candidates(tx, scope, &candidates).await?;
        for entry in &candidates {
            tx.insert_configuration_change(entry).await?;
        }
        self.change_result(tx, scope, candidates).await
    }

    /// A resource owner acknowledges values it actually applied, preserving any newer desired policy.
    pub async fn activate_in(
        &self,
        tx: &mut WriteTransaction,
        scope: &ConfigScope,
        applied: &[ConfigActivation],
    ) -> Result<ConfigChangeSet> {
        scope.require_access(tx.context())?;
        tx.validate_configuration_scope(scope).await?;
        let heads = tx
            .configuration_entries(std::slice::from_ref(scope))
            .await?;
        let entries = tx
            .configuration_entries(&super::resolution::hierarchy(scope))
            .await?;
        let desired = self.resolve(scope, tx.configuration_revision().await?, &entries, true)?;
        let active = self.resolve(scope, tx.configuration_revision().await?, &entries, false)?;
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::new();
        let recorded_at = tx.time().recorded();
        for application in applied {
            if !seen.insert(&application.key) {
                return Err(Error::invalid("configuration activation repeats a key"));
            }
            let definition = self.registry.descriptor(&application.key)?;
            definition.write_policy.require(tx.context())?;
            if definition.apply_mode != ConfigApplyMode::Restart
                || !definition.scopes.contains(&scope.level())
            {
                return Err(Error::invalid(
                    "owner activation requires a restart key in its allowed scope",
                ));
            }
            let previous = heads.iter().find(|entry| entry.key == application.key);
            compare_revision(previous, application.expected_revision)?;
            let value = self
                .registry
                .normalize(&application.key, application.applied.clone())?;
            let desired_override = previous.and_then(|entry| entry.desired.clone());
            let active_override = if desired.json(&application.key) == Some(&value) {
                desired_override.clone()
            } else {
                Some(value.clone())
            };
            if active.json(&application.key) == Some(&value)
                && previous.is_none_or(|entry| entry.active == active_override)
            {
                continue;
            }
            candidates.push(ConfigEntry {
                scope: scope.clone(),
                key: application.key.clone(),
                revision: tx.commit_seq(),
                active: active_override,
                desired: desired_override,
                apply_mode: definition.apply_mode,
                recorded_at: recorded_at.clone(),
                commit_seq: tx.commit_seq(),
            });
        }
        self.validate_candidates(tx, scope, &candidates).await?;
        for entry in &candidates {
            tx.insert_configuration_change(entry).await?;
        }
        self.change_result(tx, scope, candidates).await
    }

    async fn validate_candidates(
        &self,
        tx: &mut WriteTransaction,
        changed_scope: &ConfigScope,
        candidates: &[ConfigEntry],
    ) -> Result<()> {
        let keys = candidates
            .iter()
            .map(|entry| entry.key.as_str())
            .collect::<BTreeSet<_>>();
        let mut scopes = tx
            .configuration_scopes()
            .await?
            .into_iter()
            .filter(|scope| super::resolution::affected(changed_scope, scope))
            .collect::<BTreeSet<_>>();
        scopes.insert(changed_scope.clone());
        for scope in scopes {
            let mut entries = tx
                .configuration_entries(&super::resolution::hierarchy(&scope))
                .await?;
            entries.retain(|entry| {
                entry.scope != *changed_scope || !keys.contains(entry.key.as_str())
            });
            entries.extend_from_slice(candidates);
            let location = serde_json::to_string(&scope)?;
            self.resolve(&scope, tx.commit_seq(), &entries, false)
                .map_err(|error| {
                    Error::new(
                        error.code,
                        format!("active configuration at {location}: {}", error.message),
                    )
                })?;
            self.resolve(&scope, tx.commit_seq(), &entries, true)
                .map_err(|error| {
                    Error::new(
                        error.code,
                        format!("desired configuration at {location}: {}", error.message),
                    )
                })?;
        }
        Ok(())
    }

    async fn change_result(
        &self,
        tx: &mut WriteTransaction,
        scope: &ConfigScope,
        candidates: Vec<ConfigEntry>,
    ) -> Result<ConfigChangeSet> {
        let entries = tx
            .configuration_entries(&super::resolution::hierarchy(scope))
            .await?;
        let revision = tx.configuration_revision().await?;
        let active = self.resolve(scope, revision, &entries, false)?;
        let desired = self.resolve(scope, revision, &entries, true)?;
        let pending_keys = super::resolution::pending_keys(&active, &desired);
        let changes = candidates
            .into_iter()
            .map(|entry| ConfigChange {
                pending_restart: entry.active != entry.desired,
                entry,
            })
            .collect();
        Ok(ConfigChangeSet {
            scope: scope.clone(),
            revision,
            changes,
            active_digest: active.effective_digest,
            desired_digest: desired.effective_digest,
            pending_keys,
        })
    }
}

fn compare_revision(current: Option<&ConfigEntry>, expected: i64) -> Result<()> {
    if expected < 0 {
        return Err(Error::invalid(
            "expected configuration revision must be nonnegative",
        ));
    }
    if current.map_or(0, |entry| entry.revision) != expected {
        return Err(Error::new(
            ErrorCode::Conflict,
            format!(
                "configuration revision changed; current revision is {}",
                current.map_or(0, |entry| entry.revision)
            ),
        ));
    }
    Ok(())
}
