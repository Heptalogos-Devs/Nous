//! Typed registration and bootstrap extraction; owners supply configuration meaning.

use super::ConfigSnapshot;
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, ErrorCode, Result, validate_name};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, marker::PhantomData, sync::Arc};

#[derive(Debug)]
pub struct ConfigKey<T: 'static> {
    path: &'static str,
    marker: PhantomData<fn() -> T>,
}
impl<T: 'static> Copy for ConfigKey<T> {}
impl<T: 'static> Clone for ConfigKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: 'static> ConfigKey<T> {
    pub const fn new(path: &'static str) -> Self {
        Self {
            path,
            marker: PhantomData,
        }
    }
    pub const fn path(&self) -> &'static str {
        self.path
    }
}

pub struct ConfigDefinition<T> {
    pub owner: String,
    pub description: String,
    pub default: T,
    pub scopes: Vec<ConfigLevel>,
    pub apply_mode: ConfigApplyMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigDescriptor {
    pub key: String,
    pub owner: String,
    pub description: String,
    pub schema: Value,
    pub default: Value,
    pub scopes: Vec<ConfigLevel>,
    pub apply_mode: ConfigApplyMode,
}

type Normalizer = Arc<dyn Fn(Value) -> Result<Value> + Send + Sync>;
type SnapshotValidator = Arc<dyn Fn(&ConfigSnapshot) -> Result<()> + Send + Sync>;
struct Registered {
    descriptor: ConfigDescriptor,
    normalize: Normalizer,
}

#[derive(Clone, Default)]
pub struct ConfigRegistry {
    entries: Arc<BTreeMap<String, Registered>>,
    snapshot_validators: Arc<BTreeMap<String, SnapshotValidator>>,
}
impl ConfigRegistry {
    pub fn validate_snapshot(&self, snapshot: &ConfigSnapshot) -> Result<()> {
        for (owner, validator) in self.snapshot_validators.iter() {
            validator(snapshot).map_err(|error| {
                Error::new(
                    error.code,
                    format!("configuration owner {owner}: {}", error.message),
                )
            })?;
        }
        Ok(())
    }
    pub fn descriptor(&self, path: &str) -> Result<&ConfigDescriptor> {
        self.entries
            .get(path)
            .map(|entry| &entry.descriptor)
            .ok_or_else(|| Error::invalid(format!("unknown configuration key: {path}")))
    }
    pub fn descriptors(&self) -> impl Iterator<Item = &ConfigDescriptor> {
        self.entries.values().map(|entry| &entry.descriptor)
    }
    pub fn normalize(&self, path: &str, value: Value) -> Result<Value> {
        let entry = self
            .entries
            .get(path)
            .ok_or_else(|| Error::invalid(format!("unknown configuration key: {path}")))?;
        (entry.normalize)(value)
    }

    /// Consume each atomic registered value from the explicit TOML/JSON tree.
    /// The bootstrap parser owns document syntax; missing keys remain builtins.
    pub fn deployment_values(&self, document: &Value) -> Result<BTreeMap<String, Value>> {
        if !document.is_object() {
            return Err(Error::invalid("deployment configuration must be an object"));
        }
        let mut values = BTreeMap::new();
        self.visit("", document, &mut values)?;
        Ok(values)
    }

    fn visit(&self, path: &str, value: &Value, values: &mut BTreeMap<String, Value>) -> Result<()> {
        if self.entries.contains_key(path) {
            let normalized = self.normalize(path, value.clone())?;
            if values.insert(path.to_owned(), normalized).is_some() {
                return Err(Error::invalid(format!(
                    "deployment configuration repeats key: {path}"
                )));
            }
            return Ok(());
        }
        let prefix = format!("{path}.");
        if !path.is_empty() && !self.entries.keys().any(|key| key.starts_with(&prefix)) {
            return Err(Error::invalid(format!("unknown configuration key: {path}")));
        }
        let fields = value.as_object().ok_or_else(|| {
            Error::invalid(format!("configuration group must be an object: {path}"))
        })?;
        for (name, value) in fields {
            let child = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            if !valid_key_path(&child) {
                return Err(Error::invalid(format!(
                    "invalid configuration key: {child}"
                )));
            }
            self.visit(&child, value, values)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct ConfigRegistryBuilder {
    entries: BTreeMap<String, Registered>,
    snapshot_validators: BTreeMap<String, SnapshotValidator>,
}
impl ConfigRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register_snapshot_validator(
        &mut self,
        owner: impl Into<String>,
        validator: impl Fn(&ConfigSnapshot) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        let owner = owner.into();
        validate_name(&owner, "configuration validator owner")?;
        if self.snapshot_validators.contains_key(&owner) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "configuration owner validator is already registered",
            ));
        }
        self.snapshot_validators.insert(owner, Arc::new(validator));
        Ok(())
    }

    pub fn register<T>(
        &mut self,
        key: ConfigKey<T>,
        definition: ConfigDefinition<T>,
        validator: impl Fn(&T) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()>
    where
        T: Serialize + DeserializeOwned + JsonSchema + 'static,
    {
        let path = key.path();
        if !valid_key_path(path) {
            return Err(Error::invalid(format!("invalid configuration key: {path}")));
        }
        if self.entries.keys().any(|registered| {
            registered == path
                || registered.starts_with(&format!("{path}."))
                || path.starts_with(&format!("{registered}."))
        }) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("overlapping configuration key: {path}"),
            ));
        }
        validate_name(&definition.owner, "configuration owner")?;
        if definition.description.trim().is_empty() || definition.scopes.is_empty() {
            return Err(Error::invalid(
                "configuration definition requires description and allowed scopes",
            ));
        }
        if definition
            .scopes
            .iter()
            .enumerate()
            .any(|(index, level)| definition.scopes[..index].contains(level))
        {
            return Err(Error::invalid(
                "configuration definition repeats a scope level",
            ));
        }
        let normalize: Normalizer = Arc::new(move |value| {
            let typed: T = serde_json::from_value(value)
                .map_err(|_| Error::invalid(format!("invalid configuration value: {path}")))?;
            validator(&typed)?;
            serde_json::to_value(typed).map_err(Error::from)
        });
        let default = normalize(serde_json::to_value(&definition.default)?)?;
        let schema = serde_json::to_value(schemars::schema_for!(T))?;
        self.entries.insert(
            path.to_owned(),
            Registered {
                descriptor: ConfigDescriptor {
                    key: path.into(),
                    owner: definition.owner,
                    description: definition.description,
                    schema,
                    default,
                    scopes: definition.scopes,
                    apply_mode: definition.apply_mode,
                },
                normalize,
            },
        );
        Ok(())
    }

    pub fn finish(self) -> ConfigRegistry {
        ConfigRegistry {
            entries: Arc::new(self.entries),
            snapshot_validators: Arc::new(self.snapshot_validators),
        }
    }
}

pub(crate) fn valid_key_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}
