//! Geometry build settings and validation.
use super::*;

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
    pub(super) fn validate(&self) -> Result<(), String> {
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
    pub(super) fn validate(&self) -> Result<(), String> {
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
    pub(super) fn validate(&self) -> Result<(), String> {
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
    pub(super) fn validate(&self) -> Result<(), String> {
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
