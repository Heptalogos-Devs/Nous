use super::ReferenceDtscFieldConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ReferenceDtscConfig {
    pub field: ReferenceDtscFieldConfig,
    pub curve: ReferenceDtscCurveConfig,
    pub sparse: ReferenceDtscSparseConfig,
    pub reward: ReferenceDtscRewardConfig,
    pub trust: ReferenceDtscTrustConfig,
    pub auxiliary: ReferenceDtscAuxConfig,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscCurveConfig {
    pub strong_contact_threshold: f64,
    pub weak_contact_threshold: f64,
    pub min_closure_similarity: f64,
    #[serde(rename = "candidatePositionDecay", alias = "positionDecay")]
    pub position_decay: f64,
    pub min_geo_samples: usize,
}
impl Default for ReferenceDtscCurveConfig {
    fn default() -> Self {
        Self {
            strong_contact_threshold: 0.16,
            weak_contact_threshold: 0.06,
            min_closure_similarity: 0.2,
            position_decay: 0.035,
            min_geo_samples: 3,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscSparseConfig {
    #[serde(rename = "sparseAssociationEnabled")]
    pub enabled: bool,
    #[serde(rename = "sparseAssociationMinContacts")]
    pub min_contacts: usize,
    #[serde(rename = "sparseAssociationMinConductance")]
    pub min_conductance: f64,
    #[serde(rename = "sparseAssociationMinSimilarity")]
    pub min_similarity: f64,
    #[serde(rename = "sparseAssociationMinPotential")]
    pub min_potential: f64,
    #[serde(rename = "sparseAssociationMinClosure")]
    pub min_closure: f64,
    #[serde(rename = "sparseAssociationPairSaturation")]
    pub pair_saturation: usize,
    #[serde(rename = "sparseAssociationMaxRelief")]
    pub max_relief: f64,
}
impl Default for ReferenceDtscSparseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_contacts: 3,
            min_conductance: 0.015,
            min_similarity: 0.48,
            min_potential: 0.08,
            min_closure: 0.2,
            pair_saturation: 3,
            max_relief: 0.55,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscRewardConfig {
    pub alpha: f64,
    pub geo_reward_floor: f64,
    pub geo_reward_saturation: f64,
    pub direct_bonus_cap: f64,
    pub structural_bonus_cap: f64,
    pub thematic_bonus_cap: f64,
    pub structural_continuity_min: f64,
    pub thematic_min_potential: f64,
    pub thematic_max_isolated_ratio: f64,
    pub direct_semantic_min_potential: f64,
    pub direct_semantic_saturation: f64,
    pub direct_semantic_min_contacts: usize,
    pub direct_confidence_floor: f64,
}
impl Default for ReferenceDtscRewardConfig {
    fn default() -> Self {
        Self {
            alpha: 0.35,
            geo_reward_floor: 0.015,
            geo_reward_saturation: 0.25,
            direct_bonus_cap: 0.18,
            structural_bonus_cap: 0.1,
            thematic_bonus_cap: 0.035,
            structural_continuity_min: 0.08,
            thematic_min_potential: 0.08,
            thematic_max_isolated_ratio: 0.65,
            direct_semantic_min_potential: 0.16,
            direct_semantic_saturation: 0.35,
            direct_semantic_min_contacts: 2,
            direct_confidence_floor: 0.35,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscTrustConfig {
    #[serde(rename = "minGeoCoverageRatio", alias = "minCandidateCoverage")]
    pub min_candidate_coverage: f64,
    #[serde(rename = "minMaxGeoScore", alias = "minGeoScore")]
    pub min_geo_score: f64,
    #[serde(rename = "minGeoScoreSpread", alias = "minGeoSpread")]
    pub min_geo_spread: f64,
    pub min_strong_evidence: f64,
}
impl Default for ReferenceDtscTrustConfig {
    fn default() -> Self {
        Self {
            min_candidate_coverage: 0.2,
            min_geo_score: 0.01,
            min_geo_spread: 0.03,
            min_strong_evidence: 1.0,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscAuxConfig {
    pub enabled: bool,
    pub max_aux_bonus: f64,
    pub direct_floor_cap: f64,
    pub structural_floor_cap: f64,
    pub thematic_floor_cap: f64,
    pub min_fused_score: f64,
    pub min_closure_score: f64,
    pub min_class_evidence: f64,
    pub floor_exponent: f64,
    pub identity_anchor: ReferenceDtscIdentityConfig,
}
impl Default for ReferenceDtscAuxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_aux_bonus: 0.018,
            direct_floor_cap: 0.018,
            structural_floor_cap: 0.012,
            thematic_floor_cap: 0.006,
            min_fused_score: 0.12,
            min_closure_score: 0.55,
            min_class_evidence: 0.1,
            floor_exponent: 1.5,
            identity_anchor: ReferenceDtscIdentityConfig::default(),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscIdentityConfig {
    pub enabled: bool,
    pub min_potential: f64,
    pub min_specificity: f64,
    pub min_tag_chunk_closure: f64,
    pub min_strength: f64,
    pub floor_cap: f64,
    pub floor_exponent: f64,
}
impl Default for ReferenceDtscIdentityConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_potential: 0.8,
            min_specificity: 0.55,
            min_tag_chunk_closure: 0.35,
            min_strength: 0.55,
            floor_cap: 0.018,
            floor_exponent: 1.25,
        }
    }
}
impl ReferenceDtscConfig {
    pub(crate) fn normalized(&self) -> Self {
        let mut c = self.clone();
        c.curve.strong_contact_threshold = c.curve.strong_contact_threshold.clamp(0.0, 1.0);
        c.curve.weak_contact_threshold = c
            .curve
            .weak_contact_threshold
            .clamp(0.0, c.curve.strong_contact_threshold);
        c.curve.min_closure_similarity = c.curve.min_closure_similarity.clamp(-1.0, 1.0);
        c.curve.position_decay = c.curve.position_decay.max(0.0);
        c.curve.min_geo_samples = c.curve.min_geo_samples.max(1);
        let s = &mut c.sparse;
        s.min_contacts = s.min_contacts.max(2);
        s.min_conductance = s.min_conductance.clamp(0.0, 1.0);
        s.min_similarity = s.min_similarity.clamp(-1.0, 1.0);
        s.min_potential = s.min_potential.min(1.0).max(c.curve.weak_contact_threshold);
        s.min_closure = s.min_closure.clamp(0.0, 1.0);
        s.pair_saturation = s.pair_saturation.max(1);
        s.max_relief = s.max_relief.clamp(0.0, 0.8);
        let r = &mut c.reward;
        r.alpha = r.alpha.clamp(0.0, 1.0);
        r.geo_reward_floor = r.geo_reward_floor.clamp(0.0, 1.0);
        r.geo_reward_saturation = r
            .geo_reward_saturation
            .min(1.0)
            .max(r.geo_reward_floor + 1e-6);
        r.direct_bonus_cap = r.direct_bonus_cap.clamp(0.0, 1.0);
        r.structural_bonus_cap = r.structural_bonus_cap.clamp(0.0, r.direct_bonus_cap);
        r.thematic_bonus_cap = r.thematic_bonus_cap.clamp(0.0, r.structural_bonus_cap);
        r.structural_continuity_min = r.structural_continuity_min.clamp(0.0, 1.0);
        r.thematic_min_potential = r.thematic_min_potential.clamp(0.0, 1.0);
        r.thematic_max_isolated_ratio = r.thematic_max_isolated_ratio.clamp(0.0, 1.0);
        r.direct_semantic_min_potential = r
            .direct_semantic_min_potential
            .min(1.0)
            .max(c.curve.strong_contact_threshold);
        r.direct_semantic_saturation = r
            .direct_semantic_saturation
            .min(1.0)
            .max(r.direct_semantic_min_potential + 1e-6);
        r.direct_semantic_min_contacts = r.direct_semantic_min_contacts.max(1);
        r.direct_confidence_floor = r.direct_confidence_floor.clamp(0.0, 1.0);
        let a = &mut c.auxiliary;
        a.max_aux_bonus = a.max_aux_bonus.clamp(0.0, 0.05);
        a.direct_floor_cap = a.direct_floor_cap.clamp(0.0, a.max_aux_bonus);
        a.structural_floor_cap = a.structural_floor_cap.clamp(0.0, a.direct_floor_cap);
        a.thematic_floor_cap = a.thematic_floor_cap.clamp(0.0, a.structural_floor_cap);
        a.min_fused_score = a.min_fused_score.clamp(0.0, 1.0);
        a.min_closure_score = a.min_closure_score.clamp(0.0, 1.0);
        a.min_class_evidence = a.min_class_evidence.clamp(0.0, 1.0);
        a.floor_exponent = a.floor_exponent.clamp(0.5, 4.0);
        let id = &mut a.identity_anchor;
        id.min_potential = id.min_potential.clamp(0.0, 1.0);
        id.min_specificity = id.min_specificity.clamp(0.0, 1.0);
        id.min_tag_chunk_closure = id.min_tag_chunk_closure.clamp(0.0, 1.0);
        id.min_strength = id.min_strength.clamp(0.0, 1.0);
        id.floor_cap = id
            .floor_cap
            .clamp(0.0, r.direct_bonus_cap.min(a.max_aux_bonus));
        id.floor_exponent = id.floor_exponent.clamp(0.5, 4.0);
        c
    }
}
