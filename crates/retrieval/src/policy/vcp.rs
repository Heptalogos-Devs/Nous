// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::reference::*;
use nous_configuration::*;
use nous_core::{Error, Result};
use serde::{Deserialize, Serialize};

pub const VCP_ASSETS: ConfigKey<VcpAssetPolicy> = ConfigKey::new("retrieval.vcp.assets");
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VcpAssetPolicy {
    pub graph: ReferenceGraphConfig,
    pub intrinsic: ReferenceIntrinsicConfig,
    pub epa_anchors: usize,
    pub epa_max_basis: usize,
    pub epa_samples_per_anchor: usize,
    pub epa_candidate_limit: usize,
}
impl Default for VcpAssetPolicy {
    fn default() -> Self {
        Self {
            graph: ReferenceGraphConfig {
                forward_gain: 1.0,
                reverse_gain: 0.35,
                min_reverse_gain: 0.25,
                max_reverse_gain: 0.6,
                distance_decay: 0.08,
                reverse_inversion_guard: 0.9,
                reverse_anchor_boost: true,
                reverse_anchor_max: 1.35,
                semantic_enabled: true,
                semantic_peak: 0.65,
                semantic_sigma: 0.25,
                semantic_low_fallback: 0.1,
                outbound_mass: 0.95,
                association_reserve_mass: 0.05,
                evidence_compression: 1.0,
                wormhole_gain: 1.35,
                tension_threshold: 1.0,
                hub_exponent: 0.3,
                hub_floor: 0.55,
                hub_ceiling: 1.8,
                smoothing_ratio: 0.1,
            },
            intrinsic: ReferenceIntrinsicConfig::default(),
            epa_anchors: 64,
            epa_max_basis: 64,
            epa_samples_per_anchor: 32,
            epa_candidate_limit: 512,
        }
    }
}
impl VcpAssetPolicy {
    pub fn validate(&self) -> Result<()> {
        // JSON null cannot decode as a floating scalar: this also rejects
        // nonfinite values before they enter the numerical kernels.
        let value = serde_json::to_value(self).map_err(|e| Error::Invalid(e.to_string()))?;
        serde_json::from_value::<Self>(value).map_err(|e| Error::Invalid(e.to_string()))?;
        if self.graph.min_reverse_gain > self.graph.max_reverse_gain
            || !(8..=128).contains(&self.epa_anchors)
            || !(1..=128).contains(&self.epa_max_basis)
            || !(4..=128).contains(&self.epa_samples_per_anchor)
            || !(self.epa_anchors..=4096).contains(&self.epa_candidate_limit)
        {
            return Err(Error::Invalid("invalid VCP asset policy bounds".into()));
        }
        Ok(())
    }
}
pub(crate) fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        VCP_READOUT,
        "cognitive-retrieval",
        "VCP candidate readout numerical policy.",
        VcpReadoutPolicy::default(),
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::QueryPolicy,
        VcpReadoutPolicy::validate,
    )?;
    registry.register(
        VCP_QUERY,
        "cognitive-retrieval",
        "VCP request observation numerical policy.",
        VcpQueryPolicy::default(),
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::QueryPolicy,
        VcpQueryPolicy::validate,
    )?;
    registry.register(
        VCP_ASSETS,
        "cognitive-retrieval",
        "VCP adapter graph, intrinsic residual and EPA asset policy.",
        VcpAssetPolicy::default(),
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::ServingRebuild,
        ConfigSemanticEffect::ServingProjection,
        VcpAssetPolicy::validate,
    )
}

pub const VCP_QUERY: ConfigKey<VcpQueryPolicy> = ConfigKey::new("retrieval.vcp.query");
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VcpQueryPolicy {
    pub pyramid: ReferencePyramidConfig,
    pub gating: ReferenceGateConfig,
    pub sense: ReferenceSenseConfig,
    pub fusion: ReferenceFusionConfig,
    pub fields: ReferenceFieldConfig,
}
impl Default for VcpQueryPolicy {
    fn default() -> Self {
        Self {
            pyramid: ReferencePyramidConfig {
                max_levels: 3,
                top_k: 10,
                min_energy_ratio: 0.1,
            },
            gating: ReferenceGateConfig::default(),
            sense: ReferenceSenseConfig::default(),
            fusion: ReferenceFusionConfig::default(),
            fields: ReferenceFieldConfig {
                local_alpha: 0.15,
                transfer_alpha: 0.55,
                max_iterations: 80,
                local_tolerance: 1e-9,
                transfer_tolerance: 1e-9,
                local_mass_ratio: 0.8,
                transfer_mass_ratio: 0.9,
            },
        }
    }
}
impl VcpQueryPolicy {
    pub fn validate(&self) -> Result<()> {
        let value = serde_json::to_value(self).map_err(|e| Error::Invalid(e.to_string()))?;
        serde_json::from_value::<Self>(value).map_err(|e| Error::Invalid(e.to_string()))?;
        for range in [
            &self.gating.activation_multiplier,
            &self.gating.dynamic_boost_range,
            &self.gating.core_boost_range,
        ] {
            if range.len() != 2 || range[0] > range[1] {
                return Err(Error::Invalid(
                    "VCP gating range requires ordered endpoints".into(),
                ));
            }
        }
        if !(1..=8).contains(&self.pyramid.max_levels)
            || !(1..=128).contains(&self.pyramid.top_k)
            || !(0.0..=1.0).contains(&self.pyramid.min_energy_ratio)
            || self.sense.max_safe_hops > 64
            || self.sense.max_propagation_states < 100
            || self.fields.max_iterations == 0
        {
            return Err(Error::Invalid("invalid VCP query policy bounds".into()));
        }
        Ok(())
    }
}

pub const VCP_READOUT: ConfigKey<VcpReadoutPolicy> = ConfigKey::new("retrieval.vcp.readout");
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VcpReadoutPolicy {
    pub dtsc: ReferenceDtscConfig,
    pub v3: ReferenceReadoutConfig,
}
impl VcpReadoutPolicy {
    pub fn validate(&self) -> Result<()> {
        let value = serde_json::to_value(self).map_err(|e| Error::Invalid(e.to_string()))?;
        serde_json::from_value::<Self>(value).map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(())
    }
}
