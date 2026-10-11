// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_configuration::*;
use nous_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EpaPolicy {
    #[schemars(range(min = 2, max = 256))]
    pub minimum_samples: usize,
    #[schemars(range(min = 2, max = 256))]
    pub max_representatives: usize,
    #[schemars(range(min = 1, max = 64))]
    pub max_axes: usize,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub minimum_axis_energy: f64,
    #[schemars(range(min = 0.0, max = 0.1))]
    pub duplicate_cosine_epsilon: f64,
    #[schemars(range(min = 0.0, max = 0.1))]
    pub representative_distance_epsilon: f64,
}
const REFERENCE: &str = include_str!("../../../../config/reference/epa.json");
pub const EPA_POLICY: ConfigKey<EpaPolicy> = ConfigKey::new("retrieval.epa");
impl EpaPolicy {
    pub fn reference() -> Self {
        ReferenceProfile::parse(REFERENCE)
            .expect("EPA reference profile")
            .get(EPA_POLICY)
            .expect("EPA defaults")
    }
    pub fn validate(&self) -> Result<()> {
        if self.minimum_samples > self.max_representatives {
            return Err(Error::Invalid(
                "EPA sample threshold exceeds representative budget".into(),
            ));
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        EPA_POLICY,
        "cognitive-retrieval",
        "EPA basis policy.",
        EpaPolicy::reference(),
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::ServingRebuild,
        ConfigSemanticEffect::ServingProjection,
        EpaPolicy::validate,
    )?;
    ReferenceProfile::parse(REFERENCE)?.describe(registry)
}
