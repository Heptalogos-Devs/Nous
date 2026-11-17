//! Artifact co-occurrence and transport settings.
use super::*;

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
    pub(super) fn validate(&self) -> Result<(), String> {
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
    pub(super) fn validate(&self) -> Result<(), String> {
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
