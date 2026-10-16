//! Shared configuration registration, resolution, persistence, and snapshots.
//!
//! This crate owns configuration mechanics only. Domain meaning stays with the
//! registering owner, which resolves a snapshot into its typed policy.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod identity;
mod key;
mod profile;
pub use profile::ReferenceProfile;
mod registry;
mod service;
mod snapshot;

pub use key::*;
pub use registry::{
    CONFIG_SCHEMA_DIALECT, ConfigDeployment, ConfigDescriptor, ConfigRegistry,
    ConfigRegistryBuilder, ConfigurationBootstrapBundle,
};
pub use service::{ConfigChangeOutcome, ConfigurationService};
pub use snapshot::{ConfigSnapshot, ResolvedConfigValue};

use nous_core::Result;

pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    let bool_validator = |_: &bool| Ok(());
    registry.register(
        PROCESS_MEMORY,
        "configuration-service",
        "Whether the process provides Memory cognition.",
        true,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::RestartProcess,
        ConfigSemanticEffect::Operational,
        bool_validator,
    )?;
    registry.register(
        SUBJECT_DEFAULT_MEMORY,
        "configuration-service",
        "Default Memory capability for newly created Subjects.",
        true,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::NewSubjectsOnly,
        ConfigSemanticEffect::SubjectProvisioning,
        bool_validator,
    )?;
    registry.register(
        PROCESS_SELF,
        "configuration-service",
        "Whether the process provides Self cognition.",
        true,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::RestartProcess,
        ConfigSemanticEffect::Operational,
        bool_validator,
    )?;
    registry.register(
        SUBJECT_DEFAULT_SELF,
        "configuration-service",
        "Default Self capability for new Subjects.",
        false,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::NewSubjectsOnly,
        ConfigSemanticEffect::SubjectProvisioning,
        bool_validator,
    )?;
    registry.register(
        PROCESS_SOCIAL,
        "configuration-service",
        "Whether the process provides Social cognition.",
        true,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::RestartProcess,
        ConfigSemanticEffect::Operational,
        bool_validator,
    )?;
    registry.register(
        SUBJECT_DEFAULT_SOCIAL,
        "configuration-service",
        "Default Social capability for new Subjects.",
        false,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::NewSubjectsOnly,
        ConfigSemanticEffect::SubjectProvisioning,
        bool_validator,
    )?;
    registry.register(
        PROCESS_MOTIVATION,
        "configuration-service",
        "Whether the process provides Motivation cognition.",
        true,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::RestartProcess,
        ConfigSemanticEffect::Operational,
        bool_validator,
    )?;
    registry.register(
        SUBJECT_DEFAULT_MOTIVATION,
        "configuration-service",
        "Default Motivation capability for new Subjects.",
        false,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::NewSubjectsOnly,
        ConfigSemanticEffect::SubjectProvisioning,
        bool_validator,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
