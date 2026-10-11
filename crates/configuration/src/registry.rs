// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::key::*;
use nous_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub const CONFIG_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
pub type ConfigNormalizer = Arc<dyn Fn(&Value) -> Result<Value> + Send + Sync>;

/// Language-independent contract published by a semantic configuration owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDescriptor {
    pub path: String,
    pub owner: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub json_schema: Value,
    pub reference_default: Value,
    pub exposure: ConfigExposure,
    pub scope_policy: ConfigScopePolicy,
    pub storage_policy: ConfigStoragePolicy,
    pub apply_mode: ConfigApplyMode,
    pub semantic_effect: ConfigSemanticEffect,
    pub unit: Option<String>,
    pub sensitivity: ConfigSensitivity,
    pub reference_profile: Option<String>,
}

#[derive(Clone)]
struct RegisteredDescriptor {
    descriptor: ConfigDescriptor,
    schema: Arc<jsonschema::Validator>,
    owner_normalizer: Option<ConfigNormalizer>,
}

impl RegisteredDescriptor {
    fn normalize(&self, value: &Value) -> Result<Value> {
        let value = match &self.owner_normalizer {
            Some(normalize) => normalize(value)?,
            None => value.clone(),
        };
        self.schema.validate(&value).map_err(|error| {
            // Diagnostics expose schema location, never a possibly sensitive value.
            Error::Invalid(format!(
                "invalid configuration {} at {}",
                self.descriptor.path,
                error.instance_path()
            ))
        })?;
        Ok(value)
    }
}

#[derive(Clone)]
pub struct ConfigRegistry {
    descriptors: Arc<BTreeMap<String, RegisteredDescriptor>>,
    semantic_digest: String,
    catalog_digest: String,
}

impl ConfigRegistry {
    pub fn descriptor(&self, path: &str) -> Option<&ConfigDescriptor> {
        self.descriptors.get(path).map(|entry| &entry.descriptor)
    }

    pub fn validate(&self, path: &str, value: &Value) -> Result<()> {
        self.normalize(path, value).map(|_| ())
    }

    pub fn normalize(&self, path: &str, value: &Value) -> Result<Value> {
        self.descriptors
            .get(path)
            .ok_or_else(|| Error::Invalid(format!("unknown configuration path: {path}")))?
            .normalize(value)
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &ConfigDescriptor> {
        self.descriptors.values().map(|entry| &entry.descriptor)
    }

    pub fn reference_snapshot(&self) -> Result<crate::ConfigSnapshot> {
        let values = self
            .descriptors()
            .map(|d| {
                (
                    d.path.clone(),
                    crate::ResolvedConfigValue {
                        json: d.reference_default.clone(),
                        source: ConfigSource::ReferenceDefault,
                    },
                )
            })
            .collect();
        crate::ConfigSnapshot::new(None, 0, self.clone(), values)
    }

    pub fn semantic_digest(&self) -> &str {
        &self.semantic_digest
    }
    pub fn catalog_digest(&self) -> &str {
        &self.catalog_digest
    }
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.descriptors.keys().map(String::as_str)
    }

    /// Consume a structured value at the first owning descriptor, including maps.
    pub fn deployment_values(&self, document: &Value) -> Result<BTreeMap<String, Value>> {
        let mut output = BTreeMap::new();
        self.visit("", document, &mut output)?;
        Ok(output)
    }

    fn visit(&self, path: &str, value: &Value, output: &mut BTreeMap<String, Value>) -> Result<()> {
        if self.descriptor(path).is_some() {
            output.insert(path.to_owned(), self.normalize(path, value)?);
        } else if let Value::Object(children) = value {
            // Even an empty unknown table is an unknown configuration path.
            if !path.is_empty() && !self.paths().any(|key| key.starts_with(&format!("{path}."))) {
                return Err(Error::Invalid(format!(
                    "unknown configuration path: {path}"
                )));
            }
            for (name, child) in children {
                let child_path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                if name.contains('.') || !valid_key_path(&child_path) {
                    return Err(Error::Invalid(format!(
                        "invalid configuration path: {child_path}"
                    )));
                }
                self.visit(&child_path, child, output)?;
            }
        } else {
            return Err(Error::Invalid(format!(
                "unknown configuration path: {path}"
            )));
        }
        Ok(())
    }
}

struct PendingDescriptor {
    descriptor: ConfigDescriptor,
    owner_normalizer: Option<ConfigNormalizer>,
}

#[derive(Default)]
pub struct ConfigRegistryBuilder {
    descriptors: BTreeMap<String, PendingDescriptor>,
}

impl ConfigRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "owner registration declares the complete policy contract"
    )]
    pub fn register<T>(
        &mut self,
        key: ConfigKey<T>,
        owner: impl Into<String>,
        description: impl Into<String>,
        default: T,
        exposure: ConfigExposure,
        scope_policy: ConfigScopePolicy,
        apply_mode: ConfigApplyMode,
        semantic_effect: ConfigSemanticEffect,
        validator: impl Fn(&T) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()>
    where
        T: Clone + Serialize + DeserializeOwned + JsonSchema + Send + Sync + 'static,
    {
        let path = key.path();
        validator(&default)?;
        let reference_default =
            serde_json::to_value(default).map_err(|error| Error::Invalid(error.to_string()))?;
        let json_schema = serde_json::to_value(
            schemars::generate::SchemaSettings::draft2020_12()
                .into_generator()
                .into_root_schema_for::<T>(),
        )
        .map_err(|error| Error::Internal(error.to_string()))?;
        let descriptor = ConfigDescriptor {
            path: path.to_owned(),
            owner: owner.into(),
            title: path.to_owned(),
            description: description.into(),
            category: path.split('.').next().unwrap_or(path).to_owned(),
            json_schema,
            reference_default,
            exposure,
            scope_policy,
            storage_policy: ConfigStoragePolicy::Overrideable,
            apply_mode,
            semantic_effect,
            unit: None,
            sensitivity: ConfigSensitivity::Normal,
            reference_profile: None,
        };
        let owner_normalizer: ConfigNormalizer = Arc::new(move |value| {
            let parsed: T = serde_json::from_value(value.clone())
                .map_err(|_| Error::Invalid(format!("invalid configuration {path}")))?;
            validator(&parsed)?;
            serde_json::to_value(parsed).map_err(|error| Error::Internal(error.to_string()))
        });
        self.insert(descriptor, Some(owner_normalizer))
    }

    /// Startup-only import; there is no live registration/plugin protocol.
    pub fn import(&mut self, descriptor: ConfigDescriptor) -> Result<()> {
        self.insert(descriptor, None)
    }

    /// Numeric constraints are machine metadata and generic value validation together.
    pub fn bounds<T: 'static>(
        &mut self,
        key: ConfigKey<T>,
        minimum: impl Into<Value>,
        maximum: impl Into<Value>,
        unit: Option<&str>,
    ) -> Result<()> {
        self.describe(key.path(), |d| {
            d.json_schema["minimum"] = minimum.into();
            d.json_schema["maximum"] = maximum.into();
            d.unit = unit.map(str::to_owned);
        })
    }

    /// Owner metadata/schema bounds are finalized and revalidated before publication.
    pub fn describe(
        &mut self,
        path: &str,
        update: impl FnOnce(&mut ConfigDescriptor),
    ) -> Result<()> {
        let entry = self
            .descriptors
            .remove(path)
            .ok_or_else(|| Error::Invalid(format!("unknown configuration path: {path}")))?;
        let mut descriptor = entry.descriptor;
        update(&mut descriptor);
        if descriptor.path != path {
            return Err(Error::Invalid("descriptor identity cannot change".into()));
        }
        self.insert(descriptor, entry.owner_normalizer)
    }

    fn insert(
        &mut self,
        descriptor: ConfigDescriptor,
        owner_normalizer: Option<ConfigNormalizer>,
    ) -> Result<()> {
        let path = &descriptor.path;
        if !valid_key_path(path) || descriptor.owner.is_empty() || descriptor.category.is_empty() {
            return Err(Error::Invalid(format!(
                "invalid configuration descriptor: {path}"
            )));
        }
        if self.descriptors.keys().any(|key| {
            key == path
                || key.starts_with(&format!("{path}."))
                || path.starts_with(&format!("{key}."))
        }) {
            return Err(Error::Conflict(format!(
                "overlapping configuration descriptor: {path}"
            )));
        }
        if descriptor.storage_policy == ConfigStoragePolicy::DeploymentOnly
            && (descriptor.scope_policy != ConfigScopePolicy::SystemOnly
                || descriptor.apply_mode != ConfigApplyMode::RestartProcess)
        {
            return Err(Error::Invalid(format!(
                "deployment-only descriptor must be system/restart: {path}"
            )));
        }
        if descriptor
            .json_schema
            .get("$schema")
            .and_then(Value::as_str)
            != Some(CONFIG_SCHEMA_DIALECT)
        {
            return Err(Error::Invalid(format!(
                "configuration schema must use Draft 2020-12: {path}"
            )));
        }
        jsonschema::draft202012::meta::validate(&descriptor.json_schema)
            .map_err(|_| Error::Invalid(format!("invalid configuration schema: {path}")))?;
        let entry = PendingDescriptor {
            descriptor,
            owner_normalizer,
        };
        self.descriptors
            .insert(entry.descriptor.path.clone(), entry);
        Ok(())
    }

    pub fn finish(self) -> Result<ConfigRegistry> {
        let identity = self
            .descriptors
            .values()
            .map(|entry| crate::identity::descriptor_identity(&entry.descriptor))
            .collect::<Vec<_>>();
        let semantic_digest = blake3::hash(
            &serde_json::to_vec(&identity).map_err(|error| Error::Internal(error.to_string()))?,
        )
        .to_hex()
        .to_string();
        let catalog_digest = blake3::hash(
            &serde_json::to_vec(
                &self
                    .descriptors
                    .values()
                    .map(|entry| &entry.descriptor)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| Error::Internal(error.to_string()))?,
        )
        .to_hex()
        .to_string();
        Ok(ConfigRegistry {
            descriptors: Arc::new(
                self.descriptors
                    .into_iter()
                    .map(|(path, pending)| {
                        let schema = jsonschema::draft202012::options()
                            .should_validate_formats(true)
                            .build(&pending.descriptor.json_schema)
                            .map_err(|_| {
                                Error::Invalid(format!(
                                    "cannot compile configuration schema: {path}"
                                ))
                            })?;
                        let entry = RegisteredDescriptor {
                            descriptor: pending.descriptor,
                            schema: Arc::new(schema),
                            owner_normalizer: pending.owner_normalizer,
                        };
                        entry.normalize(&entry.descriptor.reference_default)?;
                        Ok((path, entry))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()?,
            ),
            semantic_digest,
            catalog_digest,
        })
    }
}

/// Private startup handoff; descriptors and deployment values contain references only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationBootstrapBundle {
    pub core_descriptors: Vec<ConfigDescriptor>,
    pub deployment_document: Value,
}
