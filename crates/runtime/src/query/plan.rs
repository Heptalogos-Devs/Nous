// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::bind::planned_profile_lanes;
use nous_configuration::ConfigSnapshot;
use nous_core::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct QueryPlan {
    pub concept_enrichment: super::ConceptEnrichment,
    pub retrieval_policy: super::RetrievalPolicy,
    pub enabled_lanes: Vec<EvidenceFamily>,
    pub lane_budgets: BTreeMap<EvidenceFamily, usize>,
    /// Shared upper bound for bounded non-exact providers; execution is lane-specific.
    pub candidate_limit: usize,
    pub final_validation_budget: usize,
    pub sense_cues: bool,
    pub expand_topology: bool,
    pub topology_rounds: usize,
    pub topology_nodes: usize,
    pub resource_limit: usize,
    pub materialize_evidence: bool,
    pub prefer_resource_synopsis: bool,
}

impl QueryPlan {
    pub(super) fn bind(
        query: &CognitiveQuery,
        runtime_refs: &[CognitiveRef],
        config_snapshot: &ConfigSnapshot,
    ) -> Result<Self> {
        let retrieval_policy = super::resolve_retrieval_policy(config_snapshot)?;
        let concept_enrichment = if query.is_exact_read() {
            super::ConceptEnrichment::Off
        } else {
            config_snapshot.get(super::CONCEPT_ENRICHMENT)?
        };
        Self::from_policy(query, runtime_refs, concept_enrichment, retrieval_policy)
    }

    pub fn from_policy(
        query: &CognitiveQuery,
        runtime_refs: &[CognitiveRef],
        concept_enrichment: super::ConceptEnrichment,
        retrieval_policy: super::RetrievalPolicy,
    ) -> Result<Self> {
        let mut enabled_lanes = planned_profile_lanes(query, retrieval_policy.cognitive_profile);
        include_concept_lane(query, concept_enrichment, runtime_refs, &mut enabled_lanes);
        if enabled_lanes.is_empty() && !query.requests_resources() {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "query has no enabled retrieval lane",
            ));
        }
        let lane_budgets = budget_values(query, &enabled_lanes, &retrieval_policy);
        let index = effort_index(query.effort);
        let candidate_limit = lane_budgets.values().copied().max().unwrap_or(0);
        let expand_topology = enabled_lanes.contains(&EvidenceFamily::TopologyWave)
            && retrieval_policy.cognitive_profile.requirements().topology;
        let final_validation_budget = query
            .result_need
            .limit
            .saturating_mul(retrieval_policy.validation_multiplier)
            .clamp(
                retrieval_policy.validation_min,
                retrieval_policy.validation_max,
            );
        let topology_rounds = retrieval_policy.topology_hops[index];
        let topology_nodes = retrieval_policy.topology_states[index];
        let resource_limit = retrieval_policy.resource_limits[index];
        Ok(Self {
            concept_enrichment,
            enabled_lanes,
            lane_budgets,
            candidate_limit,
            final_validation_budget,
            sense_cues: !query.is_exact_read()
                && query.capabilities.residual_sensing == RequirementStrength::Required,
            expand_topology,
            topology_rounds,
            topology_nodes,
            resource_limit,
            materialize_evidence: query.result_need.need_evidence
                && matches!(
                    query.effort,
                    CognitiveEffort::Deep | CognitiveEffort::Maximum
                ),
            prefer_resource_synopsis: query.resources.synopsis_only
                || matches!(query.exploration, ExplorationIntent::Global),
            retrieval_policy,
        })
    }

    pub fn lane_budget(&self, family: EvidenceFamily) -> usize {
        self.lane_budgets.get(&family).copied().unwrap_or(0)
    }

    pub fn lane_enabled(&self, family: EvidenceFamily) -> bool {
        self.enabled_lanes.contains(&family)
    }

    pub fn serving_need(&self, query: &CognitiveQuery) -> ServingNeed {
        if query.is_exact_read() {
            return ServingNeed::default();
        }
        let has_text = query
            .scopes()
            .into_iter()
            .flat_map(|node| &node.cues)
            .any(|cue| matches!(cue, Cue::Text(_) | Cue::Example(_)));
        let has_semantic_cue = query
            .scopes()
            .into_iter()
            .flat_map(|node| &node.cues)
            .any(|cue| matches!(cue, Cue::Concept(_)));
        ServingNeed {
            lexical: has_text && self.enabled_lanes.contains(&EvidenceFamily::Lexical),
            dense: (has_text || has_semantic_cue)
                && (self.enabled_lanes.contains(&EvidenceFamily::Dense)
                    || (self.expand_topology
                        && self
                            .retrieval_policy
                            .cognitive_profile
                            .requirements()
                            .query_embedding)),
            topology: self.expand_topology,
            concept: self.enabled_lanes.contains(&EvidenceFamily::TagDirect)
                || self.expand_topology
                || self.sense_cues,
            concept_vectors: query.capabilities.text_embedding != RequirementStrength::Forbidden
                && (self.concept_enrichment != super::ConceptEnrichment::Off
                    || (self.expand_topology
                        && self
                            .retrieval_policy
                            .cognitive_profile
                            .requirements()
                            .query_embedding)
                    || self.sense_cues),
        }
    }
}

#[derive(Debug, Default)]
pub struct WorkCycle {
    pub inspected: std::collections::HashSet<CognitiveRef>,
    pub frontier: Vec<CognitiveRef>,
}

fn effort_index(effort: CognitiveEffort) -> usize {
    match effort {
        CognitiveEffort::Light => 0,
        CognitiveEffort::Normal => 1,
        CognitiveEffort::Deep => 2,
        CognitiveEffort::Maximum => 3,
    }
}

fn include_concept_lane(
    query: &CognitiveQuery,
    enrichment: super::ConceptEnrichment,
    runtime_refs: &[CognitiveRef],
    lanes: &mut Vec<EvidenceFamily>,
) {
    if !query.is_exact_read()
        && (enrichment != super::ConceptEnrichment::Off
            || runtime_refs
                .iter()
                .any(|reference| matches!(reference, CognitiveRef::Tag(_))))
    {
        lanes.push(EvidenceFamily::TagDirect);
        lanes.sort();
        lanes.dedup();
    }
}

fn budget_values(
    query: &CognitiveQuery,
    lanes: &[EvidenceFamily],
    policy: &super::RetrievalPolicy,
) -> BTreeMap<EvidenceFamily, usize> {
    let index = effort_index(query.effort);
    let budget = query
        .result_need
        .limit
        .saturating_mul(policy.effort_multipliers[index])
        .clamp(policy.lane_min, policy.per_lane_max[index]);
    lanes.iter().copied().map(|lane| (lane, budget)).collect()
}

#[cfg(test)]
#[path = "../../tests/unit/query/plan.rs"]
mod tests;
