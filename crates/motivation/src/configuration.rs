// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use nous_configuration::*;
use nous_core::{Error, Result};
pub const CRITERION_DEPTH: ConfigKey<usize> = ConfigKey::new("motivation.criterion_max_depth");
pub const CRITERION_LEAVES: ConfigKey<usize> = ConfigKey::new("motivation.criterion_max_leaves");
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        CRITERION_DEPTH,
        "motivation",
        "Maximum typed criterion AST depth.",
        8,
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::Operational,
        |v| {
            if (1..=32).contains(v) {
                Ok(())
            } else {
                Err(Error::Invalid("criterion depth must be 1..32".into()))
            }
        },
    )?;
    registry.register(
        CRITERION_LEAVES,
        "motivation",
        "Maximum typed criterion leaf count.",
        64,
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::Operational,
        |v| {
            if (1..=1024).contains(v) {
                Ok(())
            } else {
                Err(Error::Invalid("criterion leaves must be 1..1024".into()))
            }
        },
    )?;
    Ok(())
}
