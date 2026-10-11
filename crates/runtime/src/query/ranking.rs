// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_configuration::{
    ConfigApplyMode, ConfigExposure, ConfigKey, ConfigRegistryBuilder, ConfigScopePolicy,
    ConfigSemanticEffect, ConfigSnapshot,
};
use nous_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;

pub const RRF_K_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.k");
pub const RRF_EXACT_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.exact");
pub const RRF_RUNTIME_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.runtime");
pub const RRF_ENTITY_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.entity");
pub const RRF_LEXICAL_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.lexical");
pub const RRF_DENSE_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.dense");
pub const RRF_TEMPORAL_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.temporal");
pub const RRF_SCHEMA_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.schema_direct");
pub const RRF_TAG_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.tag_direct");
pub const RRF_TOPOLOGY_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.rrf.weights.topology_wave");

pub const PREFERENCE_WEIGHT_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.preference.weight");
pub const PREFERENCE_CAP_KEY: ConfigKey<f64> = ConfigKey::new("retrieval.preference.cap");
pub const PREFERENCE_RECENCY_KEY: ConfigKey<f64> =
    ConfigKey::new("retrieval.preference.recency_seconds");

pub const QUERY_LIGHT_MULTIPLIER: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.effort_multiplier.light");
pub const QUERY_NORMAL_MULTIPLIER: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.effort_multiplier.normal");
pub const QUERY_DEEP_MULTIPLIER: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.effort_multiplier.deep");
pub const QUERY_MAXIMUM_MULTIPLIER: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.effort_multiplier.maximum");
pub const QUERY_LIGHT_MAX: ConfigKey<usize> = ConfigKey::new("retrieval.query.per_lane_max.light");
pub const QUERY_NORMAL_MAX: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.per_lane_max.normal");
pub const QUERY_DEEP_MAX: ConfigKey<usize> = ConfigKey::new("retrieval.query.per_lane_max.deep");
pub const QUERY_MAXIMUM_MAX: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.per_lane_max.maximum");
pub const QUERY_VALIDATION_MULTIPLIER: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.validation_multiplier");
pub const QUERY_VALIDATION_MIN: ConfigKey<usize> = ConfigKey::new("retrieval.query.validation_min");
pub const QUERY_VALIDATION_MAX: ConfigKey<usize> = ConfigKey::new("retrieval.query.validation_max");

pub const DEFAULT_RESULT_LIMIT: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.default_result_limit");
pub const LANE_MIN_KEY: ConfigKey<usize> = ConfigKey::new("retrieval.query.lane_min");
pub const TOPOLOGY_HOPS_LIGHT_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_hops.light");
pub const TOPOLOGY_HOPS_NORMAL_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_hops.normal");
pub const TOPOLOGY_HOPS_DEEP_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_hops.deep");
pub const TOPOLOGY_HOPS_MAXIMUM_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_hops.maximum");
pub const TOPOLOGY_STATES_LIGHT_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_states.light");
pub const TOPOLOGY_STATES_NORMAL_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_states.normal");
pub const TOPOLOGY_STATES_DEEP_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_states.deep");
pub const TOPOLOGY_STATES_MAXIMUM_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.topology_states.maximum");
pub const RESOURCE_LIMIT_LIGHT_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.resource_limit.light");
pub const RESOURCE_LIMIT_NORMAL_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.resource_limit.normal");
pub const RESOURCE_LIMIT_DEEP_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.resource_limit.deep");
pub const RESOURCE_LIMIT_MAXIMUM_KEY: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.resource_limit.maximum");
#[derive(Debug, Clone)]
pub struct RetrievalPolicy {
    pub cognitive_profile: super::CognitiveProfile,
    pub rrf_k: f64,
    pub preference_weight: f64,
    pub preference_cap: f64,
    pub preference_recency_seconds: f64,
    pub weights: BTreeMap<EvidenceFamily, f64>,
    pub effort_multipliers: [usize; 4],
    pub per_lane_max: [usize; 4],
    pub lane_min: usize,
    pub topology_hops: [usize; 4],
    pub topology_states: [usize; 4],
    pub resource_limits: [usize; 4],
    pub validation_multiplier: usize,
    pub validation_min: usize,
    pub validation_max: usize,
}

impl RetrievalPolicy {
    pub fn reference() -> Self {
        let mut registry = ConfigRegistryBuilder::new();
        register_retrieval_configuration(&mut registry).expect("reference retrieval catalog");
        resolve_retrieval_policy(
            &registry
                .finish()
                .expect("reference catalog")
                .reference_snapshot()
                .expect("reference snapshot"),
        )
        .expect("reference retrieval policy")
    }

    pub fn weight(&self, family: EvidenceFamily) -> Option<f64> {
        self.weights.get(&family).copied()
    }

    pub fn validate(&self) -> Result<()> {
        if !self.preference_weight.is_finite()
            || self.preference_weight < 0.0
            || !self.preference_cap.is_finite()
            || self.preference_cap < 0.0
            || self.preference_cap > 1.0
            || !self.preference_recency_seconds.is_finite()
            || self.preference_recency_seconds <= 0.0
            || !self.rrf_k.is_finite()
            || self.rrf_k <= 0.0
            || self
                .weights
                .values()
                .any(|value| !value.is_finite() || *value <= 0.0)
            || self.effort_multipliers.contains(&0)
            || self.lane_min == 0
            || self.per_lane_max.iter().any(|max| *max < self.lane_min)
            || self.topology_hops.contains(&0)
            || self.topology_states.contains(&0)
            || self.resource_limits.contains(&0)
            || self.per_lane_max.contains(&0)
            || self.validation_multiplier == 0
            || self.validation_min == 0
            || self.validation_max < self.validation_min
        {
            return Err(Error::Invalid("invalid retrieval policy bounds".into()));
        }
        Ok(())
    }
}

pub fn register_retrieval_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    super::cognitive_profile::register_configuration(registry)?;
    super::representation::register(registry)?;
    super::activation::register(registry)?;
    let reference = nous_configuration::ReferenceProfile::parse(include_str!(
        "../../../../config/reference/retrieval-ranking.json"
    ))?;
    for (key, description) in [
        (RRF_K_KEY, "RRF denominator constant."),
        (RRF_EXACT_KEY, "Exact lane RRF weight."),
        (RRF_RUNTIME_KEY, "Runtime lane RRF weight."),
        (RRF_ENTITY_KEY, "Entity lane RRF weight."),
        (RRF_LEXICAL_KEY, "Lexical lane RRF weight."),
        (RRF_DENSE_KEY, "Dense lane RRF weight."),
        (RRF_TEMPORAL_KEY, "Temporal lane RRF weight."),
        (RRF_SCHEMA_KEY, "Schema lane RRF weight."),
        (RRF_TAG_KEY, "Direct Tag recall RRF weight."),
        (RRF_TOPOLOGY_KEY, "Topology lane RRF weight."),
        (
            PREFERENCE_WEIGHT_KEY,
            "Soft preference contribution weight.",
        ),
        (PREFERENCE_CAP_KEY, "Soft preference aggregate cap."),
        (
            PREFERENCE_RECENCY_KEY,
            "Preference recency timescale in cognitive seconds.",
        ),
    ] {
        registry.register(
            key,
            "runtime",
            description,
            reference.get(key)?,
            ConfigExposure::Developer,
            ConfigScopePolicy::SystemOnly,
            ConfigApplyMode::Live,
            ConfigSemanticEffect::QueryPolicy,
            |value: &f64| {
                if value.is_finite() && *value > 0.0 {
                    Ok(())
                } else {
                    Err(Error::Invalid(
                        "retrieval value must be finite and positive".into(),
                    ))
                }
            },
        )?;
        registry.describe(key.path(), |d| {
            d.json_schema["exclusiveMinimum"] = serde_json::json!(0);
        })?;
    }
    for (key, description) in [
        (QUERY_LIGHT_MULTIPLIER, "Light query effort multiplier."),
        (QUERY_NORMAL_MULTIPLIER, "Normal query effort multiplier."),
        (QUERY_DEEP_MULTIPLIER, "Deep query effort multiplier."),
        (QUERY_MAXIMUM_MULTIPLIER, "Maximum query effort multiplier."),
        (QUERY_LIGHT_MAX, "Light per-lane candidate maximum."),
        (QUERY_NORMAL_MAX, "Normal per-lane candidate maximum."),
        (QUERY_DEEP_MAX, "Deep per-lane candidate maximum."),
        (QUERY_MAXIMUM_MAX, "Maximum per-lane candidate maximum."),
        (
            QUERY_VALIDATION_MULTIPLIER,
            "Final validation budget multiplier.",
        ),
        (QUERY_VALIDATION_MIN, "Final validation budget minimum."),
        (QUERY_VALIDATION_MAX, "Final validation budget maximum."),
    ] {
        registry.register(
            key,
            "runtime",
            description,
            reference.get(key)?,
            ConfigExposure::Developer,
            ConfigScopePolicy::SystemOnly,
            ConfigApplyMode::Live,
            ConfigSemanticEffect::QueryPolicy,
            |value: &usize| {
                if *value > 0 {
                    Ok(())
                } else {
                    Err(Error::Invalid("retrieval budget must be positive".into()))
                }
            },
        )?;
        registry.describe(key.path(), |d| {
            d.json_schema["minimum"] = serde_json::json!(1);
            d.unit = Some("items".into());
        })?;
    }
    register_query_allocations(registry, &reference)?;
    reference.describe(registry)?;
    registry.describe(PREFERENCE_RECENCY_KEY.path(), |d| {
        d.unit = Some("cognitive_seconds".into());
    })?;
    registry.describe(PREFERENCE_CAP_KEY.path(), |d| {
        d.json_schema["maximum"] = serde_json::json!(1);
    })?;
    Ok(())
}

fn register_query_allocations(
    registry: &mut ConfigRegistryBuilder,
    reference: &nous_configuration::ReferenceProfile,
) -> Result<()> {
    for key in [
        DEFAULT_RESULT_LIMIT,
        LANE_MIN_KEY,
        TOPOLOGY_HOPS_LIGHT_KEY,
        TOPOLOGY_HOPS_NORMAL_KEY,
        TOPOLOGY_HOPS_DEEP_KEY,
        TOPOLOGY_HOPS_MAXIMUM_KEY,
        TOPOLOGY_STATES_LIGHT_KEY,
        TOPOLOGY_STATES_NORMAL_KEY,
        TOPOLOGY_STATES_DEEP_KEY,
        TOPOLOGY_STATES_MAXIMUM_KEY,
        RESOURCE_LIMIT_LIGHT_KEY,
        RESOURCE_LIMIT_NORMAL_KEY,
        RESOURCE_LIMIT_DEEP_KEY,
        RESOURCE_LIMIT_MAXIMUM_KEY,
    ] {
        registry.register(
            key,
            "runtime",
            "Query lane and execution allocation budget.",
            reference.get(key)?,
            ConfigExposure::Developer,
            ConfigScopePolicy::SystemOnly,
            ConfigApplyMode::Live,
            ConfigSemanticEffect::QueryPolicy,
            |value| {
                if *value > 0 && *value <= 65536 {
                    Ok(())
                } else {
                    Err(Error::Invalid("query allocation must be 1..65536".into()))
                }
            },
        )?;
        registry.bounds(key, 1, 65536, Some("items"))?;
    }
    registry.bounds(
        DEFAULT_RESULT_LIMIT,
        1,
        nous_core::MAX_QUERY_RESULT_ITEMS,
        Some("items"),
    )?;
    Ok(())
}

pub fn resolve_retrieval_policy(snapshot: &ConfigSnapshot) -> Result<RetrievalPolicy> {
    let policy = RetrievalPolicy {
        cognitive_profile: snapshot.get(super::COGNITIVE_PROFILE)?,
        rrf_k: snapshot.get(RRF_K_KEY)?,
        preference_weight: snapshot.get(PREFERENCE_WEIGHT_KEY)?,
        preference_cap: snapshot.get(PREFERENCE_CAP_KEY)?,
        preference_recency_seconds: snapshot.get(PREFERENCE_RECENCY_KEY)?,
        weights: BTreeMap::from([
            (EvidenceFamily::Exact, snapshot.get(RRF_EXACT_KEY)?),
            (EvidenceFamily::Runtime, snapshot.get(RRF_RUNTIME_KEY)?),
            (EvidenceFamily::Entity, snapshot.get(RRF_ENTITY_KEY)?),
            (EvidenceFamily::Lexical, snapshot.get(RRF_LEXICAL_KEY)?),
            (EvidenceFamily::Dense, snapshot.get(RRF_DENSE_KEY)?),
            (EvidenceFamily::Temporal, snapshot.get(RRF_TEMPORAL_KEY)?),
            (EvidenceFamily::SchemaDirect, snapshot.get(RRF_SCHEMA_KEY)?),
            (EvidenceFamily::TagDirect, snapshot.get(RRF_TAG_KEY)?),
            (
                EvidenceFamily::TopologyWave,
                snapshot.get(RRF_TOPOLOGY_KEY)?,
            ),
        ]),
        effort_multipliers: [
            snapshot.get(QUERY_LIGHT_MULTIPLIER)?,
            snapshot.get(QUERY_NORMAL_MULTIPLIER)?,
            snapshot.get(QUERY_DEEP_MULTIPLIER)?,
            snapshot.get(QUERY_MAXIMUM_MULTIPLIER)?,
        ],
        per_lane_max: [
            snapshot.get(QUERY_LIGHT_MAX)?,
            snapshot.get(QUERY_NORMAL_MAX)?,
            snapshot.get(QUERY_DEEP_MAX)?,
            snapshot.get(QUERY_MAXIMUM_MAX)?,
        ],
        lane_min: snapshot.get(LANE_MIN_KEY)?,
        topology_hops: [
            snapshot.get(TOPOLOGY_HOPS_LIGHT_KEY)?,
            snapshot.get(TOPOLOGY_HOPS_NORMAL_KEY)?,
            snapshot.get(TOPOLOGY_HOPS_DEEP_KEY)?,
            snapshot.get(TOPOLOGY_HOPS_MAXIMUM_KEY)?,
        ],
        topology_states: [
            snapshot.get(TOPOLOGY_STATES_LIGHT_KEY)?,
            snapshot.get(TOPOLOGY_STATES_NORMAL_KEY)?,
            snapshot.get(TOPOLOGY_STATES_DEEP_KEY)?,
            snapshot.get(TOPOLOGY_STATES_MAXIMUM_KEY)?,
        ],
        resource_limits: [
            snapshot.get(RESOURCE_LIMIT_LIGHT_KEY)?,
            snapshot.get(RESOURCE_LIMIT_NORMAL_KEY)?,
            snapshot.get(RESOURCE_LIMIT_DEEP_KEY)?,
            snapshot.get(RESOURCE_LIMIT_MAXIMUM_KEY)?,
        ],
        validation_multiplier: snapshot.get(QUERY_VALIDATION_MULTIPLIER)?,
        validation_min: snapshot.get(QUERY_VALIDATION_MIN)?,
        validation_max: snapshot.get(QUERY_VALIDATION_MAX)?,
    };
    policy.validate()?;
    Ok(policy)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionLaneSpec {
    pub family: EvidenceFamily,
    pub weight: f64,
}

pub fn default_rrf_plan_with_policy(
    enabled: &[EvidenceFamily],
    policy: &RetrievalPolicy,
) -> Vec<FusionLaneSpec> {
    enabled
        .iter()
        .filter_map(|family| {
            let weight = policy.weight(*family)?;
            Some(FusionLaneSpec {
                family: *family,
                weight,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRankInput {
    pub reference: CognitiveRef,
    pub family_ranks: HashMap<EvidenceFamily, usize>,
    #[serde(default)]
    pub family_view_ranks: HashMap<EvidenceFamily, Vec<usize>>,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub reference: CognitiveRef,
    pub baseline_score: f64,
    pub final_score: f64,
    pub best_lane_rank: usize,
    pub exact_match: bool,
    pub families: Vec<EvidenceFamily>,
    pub variants: Vec<String>,
}

#[cfg(test)]
fn rank_candidates(
    candidates: &[CandidateRankInput],
    enabled_lanes: &[EvidenceFamily],
) -> Vec<RankedCandidate> {
    rank_candidates_with_policy(candidates, enabled_lanes, &RetrievalPolicy::reference())
}

pub fn rank_candidates_with_policy(
    candidates: &[CandidateRankInput],
    enabled_lanes: &[EvidenceFamily],
    policy: &RetrievalPolicy,
) -> Vec<RankedCandidate> {
    let enabled = default_rrf_plan_with_policy(enabled_lanes, policy);
    let denominator = enabled
        .iter()
        .map(|lane| lane.weight / (policy.rrf_k + 1.0))
        .sum::<f64>();
    let mut ranked = candidates
        .iter()
        .map(|candidate| {
            let raw = enabled
                .iter()
                .filter_map(|lane| {
                    let rank = candidate
                        .family_view_ranks
                        .get(&lane.family)
                        .and_then(|ranks| ranks.iter().copied().min())
                        .or_else(|| candidate.family_ranks.get(&lane.family).copied())?;
                    Some(lane.weight / (policy.rrf_k + rank as f64))
                })
                .sum::<f64>();
            let mut families = candidate.family_ranks.keys().copied().collect::<Vec<_>>();
            families.sort();
            let best_lane_rank = candidate
                .family_ranks
                .values()
                .copied()
                .min()
                .unwrap_or(usize::MAX);
            let score = if denominator > 0.0 {
                raw / denominator
            } else {
                0.0
            };
            RankedCandidate {
                reference: candidate.reference.clone(),
                baseline_score: score,
                final_score: score,
                best_lane_rank,
                exact_match: candidate.family_ranks.contains_key(&EvidenceFamily::Exact),
                families,
                variants: candidate.variants.clone(),
            }
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .final_score
            .total_cmp(&left.final_score)
            .then_with(|| left.best_lane_rank.cmp(&right.best_lane_rank))
            .then_with(|| right.exact_match.cmp(&left.exact_match))
            .then_with(|| left.reference.to_string().cmp(&right.reference.to_string()))
    });
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::MemoryId;

    #[test]
    fn denominator_is_plan_family_based() {
        let candidate = CandidateRankInput {
            reference: CognitiveRef::Memory(MemoryId::new()),
            family_ranks: HashMap::from([(EvidenceFamily::Lexical, 1)]),
            family_view_ranks: HashMap::new(),
            variants: Vec::new(),
        };
        let reference = candidate.reference.clone();
        let mut extra = candidate.clone();
        extra.reference = CognitiveRef::Memory(MemoryId::new());
        extra.family_ranks.insert(EvidenceFamily::Dense, 1);
        let first = rank_candidates(std::slice::from_ref(&candidate), &[EvidenceFamily::Lexical])
            [0]
        .baseline_score;
        let second = rank_candidates(&[candidate, extra], &[EvidenceFamily::Lexical])
            .into_iter()
            .find(|value| value.reference == reference)
            .unwrap()
            .baseline_score;
        assert_eq!(first, second);
    }

    #[test]
    fn multi_view_family_uses_one_best_rank_and_tie_order_is_stable() {
        let left = CandidateRankInput {
            reference: CognitiveRef::Memory(MemoryId::new()),
            family_ranks: HashMap::new(),
            family_view_ranks: HashMap::from([(EvidenceFamily::Dense, vec![7, 2, 5])]),
            variants: vec!["dense:space-a".into()],
        };
        let right = CandidateRankInput {
            reference: CognitiveRef::Memory(MemoryId::new()),
            family_ranks: HashMap::from([(EvidenceFamily::Dense, 2)]),
            family_view_ranks: HashMap::new(),
            variants: Vec::new(),
        };
        let reverse = rank_candidates(&[right.clone(), left.clone()], &[EvidenceFamily::Dense]);
        let forward = rank_candidates(&[left, right], &[EvidenceFamily::Dense]);
        let reverse_refs = reverse
            .iter()
            .map(|candidate| candidate.reference.to_string())
            .collect::<Vec<_>>();
        let forward_refs = forward
            .iter()
            .map(|candidate| candidate.reference.to_string())
            .collect::<Vec<_>>();
        assert_eq!(reverse_refs, forward_refs);
    }
}
