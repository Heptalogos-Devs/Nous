//! Current reference defaults for a jointly tuned algorithm family.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{ConfigKey, ConfigRegistryBuilder};
use nous_core::{Error, Result};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceProfile {
    pub identity: String,
    pub values: BTreeMap<String, Value>,
}
impl ReferenceProfile {
    pub fn parse(source: &str) -> Result<Self> {
        let profile: Self = serde_json::from_str(source)
            .map_err(|error| Error::Internal(format!("reference profile: {error}")))?;
        if profile.identity.is_empty() || profile.values.is_empty() {
            return Err(Error::Internal("invalid reference profile identity".into()));
        }
        Ok(profile)
    }
    pub fn get<T: DeserializeOwned>(&self, key: ConfigKey<T>) -> Result<T> {
        serde_json::from_value(
            self.values.get(key.path()).cloned().ok_or_else(|| {
                Error::Internal(format!("reference default missing: {}", key.path()))
            })?,
        )
        .map_err(|error| Error::Internal(format!("reference default {}: {error}", key.path())))
    }
    pub fn tag(&self, registry: &mut ConfigRegistryBuilder, path: &str) -> Result<()> {
        registry.describe(path, |d| {
            d.reference_profile = Some(self.identity.clone());
        })
    }
    pub fn describe(&self, registry: &mut ConfigRegistryBuilder) -> Result<()> {
        for key in self.values.keys() {
            registry.describe(key, |d| {
                d.reference_profile = Some(self.identity.clone());
            })?;
        }
        Ok(())
    }
}
