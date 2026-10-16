// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::{marker::PhantomData, str::FromStr, sync::Arc};

use nous_core::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigExposure {
    Standard,
    Advanced,
    Developer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScopePolicy {
    SystemOnly,
    SubjectOverrideAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigApplyMode {
    Live,
    ServingRebuild,
    NewSubjectsOnly,
    RestartProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSemanticEffect {
    Operational,
    QueryPolicy,
    ServingProjection,
    AuthorityFormation,
    SubjectProvisioning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    OperationOverride,
    ReferenceDefault,
    DeploymentFile,
    PersistedSystem,
    PersistedSubject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessCapabilities {
    pub memory: bool,
    pub self_cognition: bool,
    #[serde(default)]
    pub social_cognition: bool,
    #[serde(default)]
    pub motivation: bool,
}

impl Default for ProcessCapabilities {
    fn default() -> Self {
        Self {
            memory: true,
            self_cognition: true,
            social_cognition: true,
            motivation: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectCapabilities {
    pub memory: bool,
    pub self_cognition: bool,
    #[serde(default)]
    pub social_cognition: bool,
    #[serde(default)]
    pub motivation: bool,
}

impl Default for SubjectCapabilities {
    fn default() -> Self {
        Self {
            memory: true,
            self_cognition: false,
            social_cognition: false,
            motivation: false,
        }
    }
}

impl SubjectCapabilities {
    pub fn require_process(self, process: ProcessCapabilities) -> Result<Self> {
        if self.memory && !process.memory {
            return Err(Error::Invalid(
                "subject memory capability is unavailable in this process".into(),
            ));
        }
        if self.self_cognition && !process.self_cognition {
            return Err(Error::Invalid(
                "Subject Self capability is unavailable in this process".into(),
            ));
        }
        if self.social_cognition && !process.social_cognition {
            return Err(Error::Invalid(
                "Subject Social capability unavailable in this process".into(),
            ));
        }
        if self.motivation && !process.motivation {
            return Err(Error::Invalid(
                "Subject Motivation unavailable in this process".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigKey<T: 'static> {
    path: &'static str,
    marker: PhantomData<fn() -> T>,
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

impl<T: 'static> From<ConfigKey<T>> for ConfigPath {
    fn from(value: ConfigKey<T>) -> Self {
        Self(Arc::from(value.path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigPath(pub Arc<str>);

impl FromStr for ConfigPath {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        if !valid_key_path(value) {
            return Err(Error::Invalid(format!(
                "invalid configuration key: {value}"
            )));
        }
        Ok(Self(Arc::from(value)))
    }
}

pub fn valid_key_path(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    !value.is_empty()
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigStoragePolicy {
    Overrideable,
    DeploymentOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSensitivity {
    Normal,
    CredentialReference,
    SensitiveReference,
}

pub const PROCESS_MOTIVATION: ConfigKey<bool> = ConfigKey::new("capabilities.process.motivation");
pub const SUBJECT_DEFAULT_MOTIVATION: ConfigKey<bool> =
    ConfigKey::new("capabilities.subject_defaults.motivation");
pub const PROCESS_SOCIAL: ConfigKey<bool> = ConfigKey::new("capabilities.process.social_cognition");
pub const SUBJECT_DEFAULT_SOCIAL: ConfigKey<bool> =
    ConfigKey::new("capabilities.subject_defaults.social_cognition");
pub const PROCESS_SELF: ConfigKey<bool> = ConfigKey::new("capabilities.process.self_cognition");
pub const SUBJECT_DEFAULT_SELF: ConfigKey<bool> =
    ConfigKey::new("capabilities.subject_defaults.self_cognition");
pub const PROCESS_MEMORY: ConfigKey<bool> = ConfigKey::new("capabilities.process.memory");
pub const SUBJECT_DEFAULT_MEMORY: ConfigKey<bool> =
    ConfigKey::new("capabilities.subject_defaults.memory");

pub fn process_capabilities(snapshot: &crate::ConfigSnapshot) -> Result<ProcessCapabilities> {
    Ok(ProcessCapabilities {
        memory: snapshot.get(PROCESS_MEMORY)?,
        self_cognition: snapshot.get(PROCESS_SELF)?,
        social_cognition: snapshot.get(PROCESS_SOCIAL)?,
        motivation: snapshot.get(PROCESS_MOTIVATION)?,
    })
}

pub fn subject_default_capabilities(
    snapshot: &crate::ConfigSnapshot,
) -> Result<SubjectCapabilities> {
    Ok(SubjectCapabilities {
        memory: snapshot.get(SUBJECT_DEFAULT_MEMORY)?,
        self_cognition: snapshot.get(SUBJECT_DEFAULT_SELF)?,
        social_cognition: snapshot.get(SUBJECT_DEFAULT_SOCIAL)?,
        motivation: snapshot.get(SUBJECT_DEFAULT_MOTIVATION)?,
    })
}
