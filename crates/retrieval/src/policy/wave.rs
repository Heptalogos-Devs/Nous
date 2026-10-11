// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_configuration::{
    ConfigApplyMode, ConfigExposure, ConfigKey, ConfigRegistryBuilder, ConfigScopePolicy,
    ConfigSemanticEffect, ConfigSnapshot,
};
use nous_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveConfig {
    pub hub_beta: f64,
    pub hub_penalty_min: f64,
    pub hub_penalty_max: f64,
    pub outbound_budget: f64,
    pub max_hops: usize,
    pub max_states: usize,
    pub max_neighbors_per_node: usize,
    pub minimum_state_energy: f64,
    pub immediate_return_multiplier: f64,
    pub initial_budget_steps: u32,
    pub normal_edge_cost: u32,
    pub fir_gamma: f64,
    #[serde(default)]
    pub class_quality: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub seed_weights: std::collections::BTreeMap<String, f64>,
}

pub const HUB_BETA_KEY: ConfigKey<f64> = ConfigKey::new("topology.wave.hub_beta");
pub const HUB_PENALTY_MIN_KEY: ConfigKey<f64> = ConfigKey::new("topology.wave.hub_penalty_min");
pub const HUB_PENALTY_MAX_KEY: ConfigKey<f64> = ConfigKey::new("topology.wave.hub_penalty_max");
pub const OUTBOUND_BUDGET_KEY: ConfigKey<f64> = ConfigKey::new("topology.wave.outbound_budget");
pub const MAX_HOPS_KEY: ConfigKey<usize> = ConfigKey::new("topology.wave.max_hops");
pub const MAX_STATES_KEY: ConfigKey<usize> = ConfigKey::new("topology.wave.max_states");
pub const MAX_NEIGHBORS_KEY: ConfigKey<usize> =
    ConfigKey::new("topology.wave.max_neighbors_per_node");
pub const MINIMUM_STATE_ENERGY_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.minimum_state_energy");
pub const IMMEDIATE_RETURN_MULTIPLIER_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.immediate_return_multiplier");
pub const INITIAL_BUDGET_STEPS_KEY: ConfigKey<u32> =
    ConfigKey::new("topology.wave.initial_budget_steps");
pub const NORMAL_EDGE_COST_KEY: ConfigKey<u32> = ConfigKey::new("topology.wave.normal_edge_cost");
pub const FIR_GAMMA_KEY: ConfigKey<f64> = ConfigKey::new("topology.wave.fir_gamma");
pub const QUALITY_HOST_EXPLICIT_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.quality.host_explicit");
pub const QUALITY_SOURCE_EVIDENCE_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.quality.source_evidence");
pub const QUALITY_COGNITIVE_DERIVATION_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.quality.cognitive_derivation");
pub const QUALITY_DERIVED_STRUCTURE_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.quality.derived_structure");
pub const QUALITY_MEANINGFUL_USE_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.quality.meaningful_use");
pub const SEED_EXACT_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.seed_weights.exact_target");
pub const SEED_RUNTIME_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.seed_weights.runtime_situation");
pub const SEED_RELATION_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.seed_weights.relation_cue");
pub const SEED_ENTITY_KEY: ConfigKey<f64> = ConfigKey::new("topology.wave.seed_weights.entity_cue");
pub const SEED_TAG_KEY: ConfigKey<f64> = ConfigKey::new("topology.wave.seed_weights.tag_cue");
pub const SEED_LEXICAL_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.seed_weights.lexical_promoted");
pub const SEED_DENSE_KEY: ConfigKey<f64> =
    ConfigKey::new("topology.wave.seed_weights.dense_promoted");

#[expect(
    clippy::too_many_lines,
    reason = "Wave registration keeps the complete reference policy catalog in one owner boundary"
)]
pub fn register_wave_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    let reference = nous_configuration::ReferenceProfile::parse(include_str!(
        "../../../../config/reference/topology-wave.json"
    ))?;
    let finite = |value: &f64| {
        if value.is_finite() {
            Ok(())
        } else {
            Err(Error::Invalid("Wave value must be finite".into()))
        }
    };
    let positive_usize = |value: &usize| {
        if *value > 0 {
            Ok(())
        } else {
            Err(Error::Invalid("Wave bound must be positive".into()))
        }
    };
    let positive_u32 = |value: &u32| {
        if *value > 0 {
            Ok(())
        } else {
            Err(Error::Invalid("Wave bound must be positive".into()))
        }
    };
    macro_rules! float {
        ($key:expr, $default:expr, $description:expr) => {
            registry.register(
                $key,
                "cognitive-retrieval",
                $description,
                $default,
                ConfigExposure::Developer,
                ConfigScopePolicy::SystemOnly,
                ConfigApplyMode::ServingRebuild,
                ConfigSemanticEffect::ServingProjection,
                finite,
            )?;
        };
    }
    macro_rules! usize_key {
        ($key:expr, $default:expr, $description:expr) => {
            registry.register(
                $key,
                "cognitive-retrieval",
                $description,
                $default,
                ConfigExposure::Developer,
                ConfigScopePolicy::SystemOnly,
                ConfigApplyMode::ServingRebuild,
                ConfigSemanticEffect::ServingProjection,
                positive_usize,
            )?;
        };
    }
    macro_rules! u32_key {
        ($key:expr, $default:expr, $description:expr) => {
            registry.register(
                $key,
                "cognitive-retrieval",
                $description,
                $default,
                ConfigExposure::Developer,
                ConfigScopePolicy::SystemOnly,
                ConfigApplyMode::ServingRebuild,
                ConfigSemanticEffect::ServingProjection,
                positive_u32,
            )?;
        };
    }
    float!(
        HUB_BETA_KEY,
        reference.get(HUB_BETA_KEY)?,
        "Wave hub penalty exponent."
    );
    float!(
        HUB_PENALTY_MIN_KEY,
        reference.get(HUB_PENALTY_MIN_KEY)?,
        "Wave hub penalty minimum."
    );
    float!(
        HUB_PENALTY_MAX_KEY,
        reference.get(HUB_PENALTY_MAX_KEY)?,
        "Wave hub penalty maximum."
    );
    float!(
        OUTBOUND_BUDGET_KEY,
        reference.get(OUTBOUND_BUDGET_KEY)?,
        "Wave outbound conductance budget."
    );
    usize_key!(
        MAX_HOPS_KEY,
        reference.get(MAX_HOPS_KEY)?,
        "Wave maximum hops."
    );
    usize_key!(
        MAX_STATES_KEY,
        reference.get(MAX_STATES_KEY)?,
        "Wave maximum states."
    );
    usize_key!(
        MAX_NEIGHBORS_KEY,
        reference.get(MAX_NEIGHBORS_KEY)?,
        "Wave maximum neighbors per node."
    );
    float!(
        MINIMUM_STATE_ENERGY_KEY,
        reference.get(MINIMUM_STATE_ENERGY_KEY)?,
        "Wave minimum state energy."
    );
    float!(
        IMMEDIATE_RETURN_MULTIPLIER_KEY,
        reference.get(IMMEDIATE_RETURN_MULTIPLIER_KEY)?,
        "Wave immediate return multiplier."
    );
    u32_key!(
        INITIAL_BUDGET_STEPS_KEY,
        reference.get(INITIAL_BUDGET_STEPS_KEY)?,
        "Wave initial budget steps."
    );
    u32_key!(
        NORMAL_EDGE_COST_KEY,
        reference.get(NORMAL_EDGE_COST_KEY)?,
        "Wave normal edge cost."
    );
    float!(
        FIR_GAMMA_KEY,
        reference.get(FIR_GAMMA_KEY)?,
        "Wave finite impulse response gamma."
    );
    float!(
        QUALITY_HOST_EXPLICIT_KEY,
        reference.get(QUALITY_HOST_EXPLICIT_KEY)?,
        "Wave host-explicit edge quality."
    );
    float!(
        QUALITY_SOURCE_EVIDENCE_KEY,
        reference.get(QUALITY_SOURCE_EVIDENCE_KEY)?,
        "Wave source-evidence edge quality."
    );
    float!(
        QUALITY_COGNITIVE_DERIVATION_KEY,
        reference.get(QUALITY_COGNITIVE_DERIVATION_KEY)?,
        "Wave cognitive-derivation edge quality."
    );
    float!(
        QUALITY_DERIVED_STRUCTURE_KEY,
        reference.get(QUALITY_DERIVED_STRUCTURE_KEY)?,
        "Wave derived-structure edge quality."
    );
    float!(
        QUALITY_MEANINGFUL_USE_KEY,
        reference.get(QUALITY_MEANINGFUL_USE_KEY)?,
        "Wave meaningful-use edge quality."
    );
    float!(
        SEED_EXACT_KEY,
        reference.get(SEED_EXACT_KEY)?,
        "Wave exact-target seed weight."
    );
    float!(
        SEED_RUNTIME_KEY,
        reference.get(SEED_RUNTIME_KEY)?,
        "Wave runtime seed weight."
    );
    float!(
        SEED_RELATION_KEY,
        reference.get(SEED_RELATION_KEY)?,
        "Wave relation-cue seed weight."
    );
    float!(
        SEED_ENTITY_KEY,
        reference.get(SEED_ENTITY_KEY)?,
        "Wave entity-cue seed weight."
    );
    float!(
        SEED_TAG_KEY,
        reference.get(SEED_TAG_KEY)?,
        "Wave tag-cue seed weight."
    );
    float!(
        SEED_LEXICAL_KEY,
        reference.get(SEED_LEXICAL_KEY)?,
        "Wave lexical-promotion seed weight."
    );
    float!(
        SEED_DENSE_KEY,
        reference.get(SEED_DENSE_KEY)?,
        "Wave dense-promotion seed weight."
    );
    reference.describe(registry)?;
    Ok(())
}

pub fn resolve_wave_config(snapshot: &ConfigSnapshot) -> Result<WaveConfig> {
    let config = WaveConfig {
        hub_beta: snapshot.get(HUB_BETA_KEY)?,
        hub_penalty_min: snapshot.get(HUB_PENALTY_MIN_KEY)?,
        hub_penalty_max: snapshot.get(HUB_PENALTY_MAX_KEY)?,
        outbound_budget: snapshot.get(OUTBOUND_BUDGET_KEY)?,
        max_hops: snapshot.get(MAX_HOPS_KEY)?,
        max_states: snapshot.get(MAX_STATES_KEY)?,
        max_neighbors_per_node: snapshot.get(MAX_NEIGHBORS_KEY)?,
        minimum_state_energy: snapshot.get(MINIMUM_STATE_ENERGY_KEY)?,
        immediate_return_multiplier: snapshot.get(IMMEDIATE_RETURN_MULTIPLIER_KEY)?,
        initial_budget_steps: snapshot.get(INITIAL_BUDGET_STEPS_KEY)?,
        normal_edge_cost: snapshot.get(NORMAL_EDGE_COST_KEY)?,
        fir_gamma: snapshot.get(FIR_GAMMA_KEY)?,
        class_quality: std::collections::BTreeMap::from([
            (
                "host_explicit".into(),
                snapshot.get(QUALITY_HOST_EXPLICIT_KEY)?,
            ),
            (
                "source_evidence".into(),
                snapshot.get(QUALITY_SOURCE_EVIDENCE_KEY)?,
            ),
            (
                "cognitive_derivation".into(),
                snapshot.get(QUALITY_COGNITIVE_DERIVATION_KEY)?,
            ),
            (
                "derived_structure".into(),
                snapshot.get(QUALITY_DERIVED_STRUCTURE_KEY)?,
            ),
            (
                "meaningful_use".into(),
                snapshot.get(QUALITY_MEANINGFUL_USE_KEY)?,
            ),
        ]),
        seed_weights: std::collections::BTreeMap::from([
            ("exact_target".into(), snapshot.get(SEED_EXACT_KEY)?),
            ("runtime_situation".into(), snapshot.get(SEED_RUNTIME_KEY)?),
            ("relation_cue".into(), snapshot.get(SEED_RELATION_KEY)?),
            ("entity_cue".into(), snapshot.get(SEED_ENTITY_KEY)?),
            ("tag_cue".into(), snapshot.get(SEED_TAG_KEY)?),
            ("lexical_promoted".into(), snapshot.get(SEED_LEXICAL_KEY)?),
            ("dense_promoted".into(), snapshot.get(SEED_DENSE_KEY)?),
        ]),
    };
    config.validate()?;
    if config.hub_penalty_min > config.hub_penalty_max {
        return Err(Error::Invalid(
            "Wave hub penalty bounds are inverted".into(),
        ));
    }
    Ok(config)
}

impl Default for WaveConfig {
    fn default() -> Self {
        let mut registry = ConfigRegistryBuilder::new();
        register_wave_configuration(&mut registry).expect("Wave reference catalog");
        resolve_wave_config(
            &registry
                .finish()
                .expect("Wave catalog")
                .reference_snapshot()
                .expect("Wave reference snapshot"),
        )
        .expect("Wave reference policy")
    }
}

impl WaveConfig {
    pub fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.outbound_budget)
            || self.max_hops == 0
            || self.max_states == 0
            || self.max_neighbors_per_node == 0
            || !(0.0..1.0).contains(&self.immediate_return_multiplier)
            || !(0.0..1.0).contains(&self.fir_gamma)
            || self.normal_edge_cost == 0
            || self.initial_budget_steps == 0
            || self
                .class_quality
                .values()
                .any(|value| !value.is_finite() || *value < 0.0)
            || self
                .seed_weights
                .values()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(Error::Invalid("invalid WaveConfig bounds".into()));
        }
        Ok(())
    }
}
