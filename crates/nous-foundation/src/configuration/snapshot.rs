//! Frozen resolved values; inheritance and changes belong to Configuration service.

use super::registry::{ConfigKey, valid_key_path};
use nous_domain::{ConfigScope, Error, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigSource {
    Builtin,
    Deployment,
    Override { scope: ConfigScope, revision: i64 },
    Operation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedConfigValue {
    pub json: Value,
    pub source: ConfigSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    pub scope: ConfigScope,
    pub revision: i64,
    pub desired: bool,
    pub effective_digest: String,
    values: BTreeMap<String, ResolvedConfigValue>,
}
impl ConfigSnapshot {
    pub(crate) fn new(
        scope: ConfigScope,
        revision: i64,
        desired: bool,
        values: BTreeMap<String, ResolvedConfigValue>,
    ) -> Result<Self> {
        validate_scope(&scope)?;
        if revision < 0 {
            return Err(Error::invalid(
                "configuration snapshot revision must be nonnegative",
            ));
        }
        for (key, value) in &values {
            if !valid_key_path(key) {
                return Err(Error::invalid(format!(
                    "invalid resolved configuration key: {key}"
                )));
            }
            if let ConfigSource::Override { scope, revision } = &value.source {
                validate_scope(scope)?;
                if *revision <= 0 {
                    return Err(Error::invalid(
                        "configuration override source requires a positive revision",
                    ));
                }
            }
        }
        let effective = values
            .iter()
            .map(|(key, value)| (key.as_str(), &value.json))
            .collect::<BTreeMap<_, _>>();
        let effective_digest = digest(&effective)?;
        Ok(Self {
            scope,
            revision,
            desired,
            effective_digest,
            values,
        })
    }

    pub fn values(&self) -> &BTreeMap<String, ResolvedConfigValue> {
        &self.values
    }

    pub fn get<T: DeserializeOwned + 'static>(&self, key: ConfigKey<T>) -> Result<T> {
        let value = self.json(key.path()).ok_or_else(|| {
            Error::internal(format!("configuration key is not resolved: {}", key.path()))
        })?;
        serde_json::from_value(value.clone()).map_err(|_| {
            Error::internal(format!(
                "resolved configuration has an invalid type: {}",
                key.path()
            ))
        })
    }
    pub fn json(&self, path: &str) -> Option<&Value> {
        self.values.get(path).map(|value| &value.json)
    }
    pub fn source(&self, path: &str) -> Option<&ConfigSource> {
        self.values.get(path).map(|value| &value.source)
    }

    /// Bind just the caller's consumed policy values, independent of key order or provenance.
    pub fn digest_for(&self, keys: &[&str]) -> Result<String> {
        let selected = keys
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|key| {
                self.json(key).map(|value| (key, value)).ok_or_else(|| {
                    Error::invalid(format!("configuration key is not resolved: {key}"))
                })
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        digest(&selected)
    }
}

fn digest(value: &impl Serialize) -> Result<String> {
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}
fn validate_scope(scope: &ConfigScope) -> Result<()> {
    match scope {
        ConfigScope::Instance => Ok(()),
        ConfigScope::Namespace { namespace_id } => namespace_id.validate(),
        ConfigScope::Subject {
            namespace_id,
            subject_id,
        } => {
            namespace_id.validate()?;
            subject_id.validate()
        }
    }
}
