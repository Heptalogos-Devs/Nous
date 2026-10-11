// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{ConfigRegistry, ConfigSource};
use nous_core::{Error, Result, SubjectId};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{collections::BTreeMap, fmt, sync::Arc};

#[derive(Debug, Clone)]
pub struct ResolvedConfigValue {
    pub json: Value,
    pub source: ConfigSource,
}

#[derive(Clone)]
pub struct ConfigSnapshot {
    pub subject_id: Option<SubjectId>,
    pub revision: i64,
    pub catalog_digest: String,
    pub effective_digest: String,
    values: Arc<BTreeMap<String, ResolvedConfigValue>>,
    registry: ConfigRegistry,
}

impl fmt::Debug for ConfigSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigSnapshot")
            .field("subject_id", &self.subject_id)
            .field("revision", &self.revision)
            .field("catalog_digest", &self.catalog_digest)
            .field("effective_digest", &self.effective_digest)
            .field("values", &self.values)
            .finish()
    }
}

impl ConfigSnapshot {
    pub(crate) fn new(
        subject_id: Option<SubjectId>,
        revision: i64,
        registry: ConfigRegistry,
        mut values: BTreeMap<String, ResolvedConfigValue>,
    ) -> Result<Self> {
        for (path, value) in &mut values {
            value.json = registry.normalize(path, &value.json)?;
        }
        let digest_input = values
            .iter()
            .map(|(key, value)| serde_json::json!({"key": key, "value": value.json}))
            .collect::<Vec<_>>();
        let effective_digest = blake3::hash(
            serde_json::to_string(&(registry.semantic_digest(), &digest_input))
                .map_err(|error| Error::Internal(error.to_string()))?
                .as_bytes(),
        )
        .to_hex()
        .to_string();
        Ok(Self {
            subject_id,
            revision,
            catalog_digest: registry.catalog_digest().to_owned(),
            effective_digest,
            values: Arc::new(values),
            registry,
        })
    }

    pub fn get<T>(&self, key: crate::ConfigKey<T>) -> Result<T>
    where
        T: DeserializeOwned + 'static,
    {
        self.get_path(key.path())
    }

    pub fn get_path<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let value = self
            .values
            .get(path)
            .ok_or_else(|| Error::Internal(format!("configuration key is not resolved: {path}")))?;
        serde_json::from_value(value.json.clone())
            .map_err(|error| Error::Internal(format!("resolved configuration {path}: {error}")))
    }

    pub fn json(&self, path: &str) -> Option<&Value> {
        self.values.get(path).map(|value| &value.json)
    }

    pub fn source(&self, path: &str) -> Option<ConfigSource> {
        self.values.get(path).map(|value| value.source)
    }

    pub fn digest_for(&self, keys: &[&str]) -> Result<String> {
        let values = keys
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|key| {
                let value = self.json(key).ok_or_else(|| {
                    Error::Invalid(format!("configuration key is not resolved: {key}"))
                })?;
                let descriptor = self.registry.descriptor(key).expect("snapshot descriptor");
                Ok(serde_json::json!({"key": key, "value": value, "contract": crate::identity::descriptor_identity(descriptor)}))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(blake3::hash(
            serde_json::to_string(&values)
                .map_err(|error| Error::Internal(error.to_string()))?
                .as_bytes(),
        )
        .to_hex()
        .to_string())
    }

    /// An immutable query-local policy overlay; never writes desired/active configuration.
    pub fn query_override<T: serde::Serialize>(
        &self,
        key: crate::ConfigKey<T>,
        value: T,
    ) -> Result<Self> {
        let descriptor = self
            .registry
            .descriptor(key.path())
            .ok_or_else(|| Error::Invalid("query override key is not registered".into()))?;
        if descriptor.apply_mode != crate::ConfigApplyMode::Live
            || descriptor.semantic_effect != crate::ConfigSemanticEffect::QueryPolicy
        {
            return Err(Error::Invalid(
                "operation override is limited to live query policy".into(),
            ));
        }
        let value =
            serde_json::to_value(value).map_err(|error| Error::Invalid(error.to_string()))?;
        self.registry.validate(key.path(), &value)?;
        let mut values = self.values.as_ref().clone();
        values.insert(
            key.path().into(),
            ResolvedConfigValue {
                json: value,
                source: ConfigSource::OperationOverride,
            },
        );
        Self::new(
            self.subject_id,
            self.revision,
            self.registry.clone(),
            values,
        )
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &crate::ConfigDescriptor> {
        self.registry.descriptors()
    }
}
