//! Typed build configuration shared by execution and packaged schema/defaults.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct GeometryLocal {
    #[schemars(range(min = 1.0, max = 256.0))]
    pub max_neighbors: usize,
    #[schemars(range(min = 1.0, max = 256.0))]
    pub min_neighbors: usize,
    #[schemars(range(min = 1.0, max = 32.0))]
    pub max_basis: usize,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_gain: f64,
    #[schemars(range(min = 0.0, max = 4.0))]
    pub position_decay: f64,
    pub semantic_gate: SemanticGate,
    pub anchor_gain: AnchorGain,
}
impl Default for GeometryLocal {
    fn default() -> Self {
        Self {
            max_neighbors: 48,
            min_neighbors: 3,
            max_basis: 4,
            min_gain: 0.015,
            position_decay: 0.15,
            semantic_gate: Default::default(),
            anchor_gain: Default::default(),
        }
    }
}
impl GeometryLocal {
    fn validate(&self) -> Result<(), String> {
        if self.max_neighbors < 1 || self.max_neighbors > 256 {
            return Err("GeometryLocal.max_neighbors outside supported range".into());
        }
        if self.min_neighbors < 1 || self.min_neighbors > 256 {
            return Err("GeometryLocal.min_neighbors outside supported range".into());
        }
        if self.max_basis < 1 || self.max_basis > 32 {
            return Err("GeometryLocal.max_basis outside supported range".into());
        }
        if !self.min_gain.is_finite() || self.min_gain < 0.0 || self.min_gain > 1.0 {
            return Err("GeometryLocal.min_gain outside supported range".into());
        }
        if !self.position_decay.is_finite()
            || self.position_decay < 0.0
            || self.position_decay > 4.0
        {
            return Err("GeometryLocal.position_decay outside supported range".into());
        }
        self.semantic_gate.validate()?;
        self.anchor_gain.validate()?;
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SemanticGate {
    pub enabled: bool,
    #[schemars(range(min = -1.0, max = 1.0))]
    pub peak: f64,
    #[schemars(range(min = 0.02, max = 2.0))]
    pub sigma: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub floor: f64,
    #[schemars(range(min = -1.0, max = 1.0))]
    pub hard_floor: f64,
}
impl Default for SemanticGate {
    fn default() -> Self {
        Self {
            enabled: true,
            peak: 0.65,
            sigma: 0.25,
            floor: 0.35,
            hard_floor: -1.0,
        }
    }
}
impl SemanticGate {
    fn validate(&self) -> Result<(), String> {
        if !self.peak.is_finite() || self.peak < -1.0 || self.peak > 1.0 {
            return Err("SemanticGate.peak outside supported range".into());
        }
        if !self.sigma.is_finite() || self.sigma < 0.02 || self.sigma > 2.0 {
            return Err("SemanticGate.sigma outside supported range".into());
        }
        if !self.floor.is_finite() || self.floor < 0.0 || self.floor > 1.0 {
            return Err("SemanticGate.floor outside supported range".into());
        }
        if !self.hard_floor.is_finite() || self.hard_floor < -1.0 || self.hard_floor > 1.0 {
            return Err("SemanticGate.hard_floor outside supported range".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct AnchorGain {
    #[schemars(range(min = 0.0, max = 4.0))]
    pub base: f64,
    #[schemars(range(min = 0.0, max = 4.0))]
    pub scale: f64,
    #[schemars(range(min = 0.1, max = 8.0))]
    pub gamma: f64,
    #[schemars(range(min = 0.0, max = 4.0))]
    pub min: f64,
    #[schemars(range(min = 0.0, max = 8.0))]
    pub max: f64,
}
impl Default for AnchorGain {
    fn default() -> Self {
        Self {
            base: 0.75,
            scale: 1.25,
            gamma: 1.0,
            min: 0.5,
            max: 2.0,
        }
    }
}
impl AnchorGain {
    fn validate(&self) -> Result<(), String> {
        if !self.base.is_finite() || self.base < 0.0 || self.base > 4.0 {
            return Err("AnchorGain.base outside supported range".into());
        }
        if !self.scale.is_finite() || self.scale < 0.0 || self.scale > 4.0 {
            return Err("AnchorGain.scale outside supported range".into());
        }
        if !self.gamma.is_finite() || self.gamma < 0.1 || self.gamma > 8.0 {
            return Err("AnchorGain.gamma outside supported range".into());
        }
        if !self.min.is_finite() || self.min < 0.0 || self.min > 4.0 {
            return Err("AnchorGain.min outside supported range".into());
        }
        if !self.max.is_finite() || self.max < 0.0 || self.max > 8.0 {
            return Err("AnchorGain.max outside supported range".into());
        }
        if self.min > self.max {
            return Err("anchor min exceeds max".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct GeometryGlobal {
    #[schemars(range(min = 1.0, max = 128.0))]
    pub k_max: usize,
    #[schemars(range(min = 1.0, max = 128.0))]
    pub k_sem: usize,
    #[schemars(range(min = 1.0, max = 256.0))]
    pub star_neighbors: usize,
    #[schemars(range(min = 0.0, max = 1000000000000.0))]
    pub min_support: f64,
    #[schemars(range(min = 0.0, max = 4.0))]
    pub support_exponent: f64,
    #[schemars(range(min = 0.0, max = 4.0))]
    pub residual_exponent: f64,
    pub prior_mode: String,
    pub candidate_mode: String,
    #[schemars(range(min = 1.0, max = 4096.0))]
    pub candidate_pool: usize,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub star_mix: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_gain: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub epsilon: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub knee_delta: f64,
}
impl Default for GeometryGlobal {
    fn default() -> Self {
        Self {
            k_max: 32,
            k_sem: 8,
            star_neighbors: 48,
            min_support: 3.0,
            support_exponent: 0.65,
            residual_exponent: 0.35,
            prior_mode: "support".into(),
            candidate_mode: "star".into(),
            candidate_pool: 512,
            star_mix: 0.8,
            min_gain: 0.02,
            epsilon: 0.1,
            knee_delta: 0.0,
        }
    }
}
impl GeometryGlobal {
    fn validate(&self) -> Result<(), String> {
        if self.k_max < 1 || self.k_max > 128 {
            return Err("GeometryGlobal.k_max outside supported range".into());
        }
        if self.k_sem < 1 || self.k_sem > 128 {
            return Err("GeometryGlobal.k_sem outside supported range".into());
        }
        if self.star_neighbors < 1 || self.star_neighbors > 256 {
            return Err("GeometryGlobal.star_neighbors outside supported range".into());
        }
        if !self.min_support.is_finite()
            || self.min_support < 0.0
            || self.min_support > 1000000000000.0
        {
            return Err("GeometryGlobal.min_support outside supported range".into());
        }
        if !self.support_exponent.is_finite()
            || self.support_exponent < 0.0
            || self.support_exponent > 4.0
        {
            return Err("GeometryGlobal.support_exponent outside supported range".into());
        }
        if !self.residual_exponent.is_finite()
            || self.residual_exponent < 0.0
            || self.residual_exponent > 4.0
        {
            return Err("GeometryGlobal.residual_exponent outside supported range".into());
        }
        if self.candidate_pool < 1 || self.candidate_pool > 4096 {
            return Err("GeometryGlobal.candidate_pool outside supported range".into());
        }
        if !self.star_mix.is_finite() || self.star_mix < 0.0 || self.star_mix > 1.0 {
            return Err("GeometryGlobal.star_mix outside supported range".into());
        }
        if !self.min_gain.is_finite() || self.min_gain < 0.0 || self.min_gain > 1.0 {
            return Err("GeometryGlobal.min_gain outside supported range".into());
        }
        if !self.epsilon.is_finite() || self.epsilon < 0.0 || self.epsilon > 1.0 {
            return Err("GeometryGlobal.epsilon outside supported range".into());
        }
        if !self.knee_delta.is_finite() || self.knee_delta < 0.0 || self.knee_delta > 1.0 {
            return Err("GeometryGlobal.knee_delta outside supported range".into());
        }
        if self.k_sem > self.k_max {
            return Err("kSem exceeds kMax".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct OrderedCooccurrence {
    #[schemars(range(min = 0.0, max = 4.0))]
    pub forward_gain: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub reverse_gain: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_reverse_gain: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub max_reverse_gain: f64,
    #[schemars(range(min = 0.0, max = 10.0))]
    pub distance_decay: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub reverse_inversion_guard: f64,
    pub reverse_anchor_boost: bool,
    #[schemars(range(min = 1.0, max = 8.0))]
    pub reverse_anchor_max: f64,
    pub semantic_gain_enabled: bool,
    #[schemars(range(min = -1.0, max = 1.0))]
    pub semantic_gain_peak: f64,
    #[schemars(range(min = 0.001, max = 2.0))]
    pub semantic_gain_sigma: f64,
    #[schemars(range(min = -1.0, max = 1.0))]
    pub semantic_gain_low_sim_fallback: f64,
    #[schemars(range(min = 0.0, max = 4.0))]
    pub provenance_reverse_gain: f64,
    #[schemars(range(min = 0.0, max = 10.0))]
    pub provenance_distance_decay: f64,
}
impl Default for OrderedCooccurrence {
    fn default() -> Self {
        Self {
            forward_gain: 1.0,
            reverse_gain: 0.42,
            min_reverse_gain: 0.25,
            max_reverse_gain: 0.7,
            distance_decay: 0.0,
            reverse_inversion_guard: 0.95,
            reverse_anchor_boost: false,
            reverse_anchor_max: 1.5,
            semantic_gain_enabled: false,
            semantic_gain_peak: 0.65,
            semantic_gain_sigma: 0.25,
            semantic_gain_low_sim_fallback: 0.1,
            provenance_reverse_gain: 0.35,
            provenance_distance_decay: 0.08,
        }
    }
}
impl OrderedCooccurrence {
    fn validate(&self) -> Result<(), String> {
        if !self.forward_gain.is_finite() || self.forward_gain < 0.0 || self.forward_gain > 4.0 {
            return Err("OrderedCooccurrence.forward_gain outside supported range".into());
        }
        if !self.reverse_gain.is_finite() || self.reverse_gain < 0.0 || self.reverse_gain > 1.0 {
            return Err("OrderedCooccurrence.reverse_gain outside supported range".into());
        }
        if !self.min_reverse_gain.is_finite()
            || self.min_reverse_gain < 0.0
            || self.min_reverse_gain > 1.0
        {
            return Err("OrderedCooccurrence.min_reverse_gain outside supported range".into());
        }
        if !self.max_reverse_gain.is_finite()
            || self.max_reverse_gain < 0.0
            || self.max_reverse_gain > 1.0
        {
            return Err("OrderedCooccurrence.max_reverse_gain outside supported range".into());
        }
        if !self.distance_decay.is_finite()
            || self.distance_decay < 0.0
            || self.distance_decay > 10.0
        {
            return Err("OrderedCooccurrence.distance_decay outside supported range".into());
        }
        if !self.reverse_inversion_guard.is_finite()
            || self.reverse_inversion_guard < 0.0
            || self.reverse_inversion_guard > 1.0
        {
            return Err(
                "OrderedCooccurrence.reverse_inversion_guard outside supported range".into(),
            );
        }
        if !self.reverse_anchor_max.is_finite()
            || self.reverse_anchor_max < 1.0
            || self.reverse_anchor_max > 8.0
        {
            return Err("OrderedCooccurrence.reverse_anchor_max outside supported range".into());
        }
        if !self.semantic_gain_peak.is_finite()
            || self.semantic_gain_peak < -1.0
            || self.semantic_gain_peak > 1.0
        {
            return Err("OrderedCooccurrence.semantic_gain_peak outside supported range".into());
        }
        if !self.semantic_gain_sigma.is_finite()
            || self.semantic_gain_sigma < 0.001
            || self.semantic_gain_sigma > 2.0
        {
            return Err("OrderedCooccurrence.semantic_gain_sigma outside supported range".into());
        }
        if !self.semantic_gain_low_sim_fallback.is_finite()
            || self.semantic_gain_low_sim_fallback < -1.0
            || self.semantic_gain_low_sim_fallback > 1.0
        {
            return Err(
                "OrderedCooccurrence.semantic_gain_low_sim_fallback outside supported range".into(),
            );
        }
        if !self.provenance_reverse_gain.is_finite()
            || self.provenance_reverse_gain < 0.0
            || self.provenance_reverse_gain > 4.0
        {
            return Err(
                "OrderedCooccurrence.provenance_reverse_gain outside supported range".into(),
            );
        }
        if !self.provenance_distance_decay.is_finite()
            || self.provenance_distance_decay < 0.0
            || self.provenance_distance_decay > 10.0
        {
            return Err(
                "OrderedCooccurrence.provenance_distance_decay outside supported range".into(),
            );
        }
        if self.min_reverse_gain > self.max_reverse_gain {
            return Err("reverse gain range is inverted".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Transport {
    #[schemars(range(min = 0.01, max = 0.999999))]
    pub outbound_mass: f64,
    #[schemars(range(min = 0.0, max = 0.999999))]
    pub association_reserve_mass: f64,
    #[schemars(range(min = 0.01, max = 100.0))]
    pub evidence_compression: f64,
    #[schemars(range(min = 1.0, max = 10.0))]
    pub wormhole_gain: f64,
    #[schemars(range(min = 0.0, max = 100.0))]
    pub tension_threshold: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub hub_penalty_exponent: f64,
    #[schemars(range(min = 0.05, max = 1.0))]
    pub hub_penalty_floor: f64,
    #[schemars(range(min = 1.0, max = 4.0))]
    pub hub_penalty_ceiling: f64,
    #[schemars(range(min = 0.01, max = 2.0))]
    pub hub_smoothing_ratio: f64,
}
impl Default for Transport {
    fn default() -> Self {
        Self {
            outbound_mass: 0.95,
            association_reserve_mass: 0.05,
            evidence_compression: 1.0,
            wormhole_gain: 1.35,
            tension_threshold: 1.0,
            hub_penalty_exponent: 0.3,
            hub_penalty_floor: 0.55,
            hub_penalty_ceiling: 1.8,
            hub_smoothing_ratio: 0.1,
        }
    }
}
impl Transport {
    fn validate(&self) -> Result<(), String> {
        if !self.outbound_mass.is_finite()
            || self.outbound_mass < 0.01
            || self.outbound_mass > 0.999999
        {
            return Err("Transport.outbound_mass outside supported range".into());
        }
        if !self.association_reserve_mass.is_finite()
            || self.association_reserve_mass < 0.0
            || self.association_reserve_mass > 0.999999
        {
            return Err("Transport.association_reserve_mass outside supported range".into());
        }
        if !self.evidence_compression.is_finite()
            || self.evidence_compression < 0.01
            || self.evidence_compression > 100.0
        {
            return Err("Transport.evidence_compression outside supported range".into());
        }
        if !self.wormhole_gain.is_finite() || self.wormhole_gain < 1.0 || self.wormhole_gain > 10.0
        {
            return Err("Transport.wormhole_gain outside supported range".into());
        }
        if !self.tension_threshold.is_finite()
            || self.tension_threshold < 0.0
            || self.tension_threshold > 100.0
        {
            return Err("Transport.tension_threshold outside supported range".into());
        }
        if !self.hub_penalty_exponent.is_finite()
            || self.hub_penalty_exponent < 0.0
            || self.hub_penalty_exponent > 1.0
        {
            return Err("Transport.hub_penalty_exponent outside supported range".into());
        }
        if !self.hub_penalty_floor.is_finite()
            || self.hub_penalty_floor < 0.05
            || self.hub_penalty_floor > 1.0
        {
            return Err("Transport.hub_penalty_floor outside supported range".into());
        }
        if !self.hub_penalty_ceiling.is_finite()
            || self.hub_penalty_ceiling < 1.0
            || self.hub_penalty_ceiling > 4.0
        {
            return Err("Transport.hub_penalty_ceiling outside supported range".into());
        }
        if !self.hub_smoothing_ratio.is_finite()
            || self.hub_smoothing_ratio < 0.01
            || self.hub_smoothing_ratio > 2.0
        {
            return Err("Transport.hub_smoothing_ratio outside supported range".into());
        }
        if self.association_reserve_mass > self.outbound_mass {
            return Err("reserve exceeds outbound mass".into());
        }
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct GeometryConfig {
    pub local: GeometryLocal,
    pub global: GeometryGlobal,
}
impl GeometryConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.local.validate()?;
        self.global.validate()?;
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ArtifactConfig {
    pub ordered_cooccurrence: OrderedCooccurrence,
    pub v9: Transport,
}
impl ArtifactConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.ordered_cooccurrence.validate()?;
        self.v9.validate()?;
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct BuildConfig {
    /// Maximum occurrence pairs in each construction pass; null computes all.
    pub pair_work_limit: Option<u64>,
    pub geometry: GeometryConfig,
    pub artifact: ArtifactConfig,
}
impl BuildConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.geometry.validate()?;
        self.artifact.validate()?;
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct QueryControls {
    #[schemars(range(min = 1, max = 1000000))]
    /// Maximum different candidate rows inspected per grouped vector lane.
    pub group_scan_limit: usize,
    #[schemars(range(min = 1, max = 10000))]
    pub top_k: usize,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub thinking_min_closure: f64,
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub rrf_k: f64,
}
impl Default for QueryControls {
    fn default() -> Self {
        Self {
            top_k: 10,
            group_scan_limit: 4096,
            thinking_min_closure: crate::thinking::default_closure(),
            rrf_k: 60.0,
        }
    }
}
impl QueryControls {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=1000000).contains(&self.group_scan_limit) {
            return Err("group_scan_limit must be in 1..1000000".into());
        }
        if !(1..=10000).contains(&self.top_k) {
            return Err("top_k must be in 1..10000".into());
        }
        if !self.thinking_min_closure.is_finite()
            || !(0.0..=1.0).contains(&self.thinking_min_closure)
        {
            return Err("thinking_min_closure must be finite and in 0..1".into());
        }
        if !self.rrf_k.is_finite() || self.rrf_k <= 0.0 {
            return Err("rrf_k must be finite and positive".into());
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContextControls {
    #[schemars(range(min = -1.0, max = 1.0))]
    pub segment_threshold: f64,
    #[schemars(range(min = 1, max = 10000))]
    pub window: usize,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub decay: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub context_mix: f64,
    #[schemars(extend("additionalProperties" = serde_json::json!({"type":"number","minimum":0.0})))]
    pub role_weights: std::collections::BTreeMap<String, f64>,
    #[schemars(range(min = 0.0))]
    pub default_role_weight: f64,
}
impl Default for ContextControls {
    fn default() -> Self {
        Self {
            window: 10,
            segment_threshold: 0.70,
            decay: 0.75,
            context_mix: 0.2,
            role_weights: [
                ("user", 1.0),
                ("assistant", 0.5),
                ("work", 1.0),
                ("ego", 0.25),
                ("source", 1.0),
            ]
            .into_iter()
            .map(|(role, weight)| (role.into(), weight))
            .collect(),
            default_role_weight: 1.0,
        }
    }
}
