// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::BoundQuery;
use super::bind::planned_profile_lanes;
use nous_core::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct QueryPlan {
    pub concept_enrichment: super::ConceptEnrichment,
    pub cognitive_profile: super::CognitiveProfile,
    pub enabled_lanes: Vec<EvidenceFamily>,
    pub lane_budgets: BTreeMap<EvidenceFamily, usize>,
    /// Shared upper bound for bounded non-exact providers; execution is lane-specific.
    pub candidate_limit: usize,
    pub final_validation_budget: usize,
    pub expand_topology: bool,
    pub topology_rounds: usize,
    pub topology_nodes: usize,
    pub resource_limit: usize,
    pub materialize_evidence: bool,
    pub prefer_resource_synopsis: bool,
}

impl QueryPlan {
    pub fn for_bound_query(bound: &BoundQuery) -> Self {
        let mut plan = Self::from_parts(
            &bound.source_query,
            bound.enabled_lanes.clone(),
            bound.lane_budgets.clone(),
            &bound.retrieval_policy,
        );
        plan.concept_enrichment = bound.concept_enrichment;
        plan
    }

    fn from_parts(
        query: &CognitiveQuery,
        enabled_lanes: Vec<EvidenceFamily>,
        lane_budgets: BTreeMap<EvidenceFamily, usize>,
        retrieval_policy: &super::RetrievalPolicy,
    ) -> Self {
        let index = effort_index(query.effort);
        let candidate_limit = lane_budgets.values().copied().max().unwrap_or(0);
        Self {
            concept_enrichment: super::ConceptEnrichment::Off,
            cognitive_profile: retrieval_policy.cognitive_profile,
            enabled_lanes: enabled_lanes.clone(),
            lane_budgets,
            candidate_limit,
            final_validation_budget: query
                .result_need
                .limit
                .saturating_mul(retrieval_policy.validation_multiplier)
                .clamp(
                    retrieval_policy.validation_min,
                    retrieval_policy.validation_max,
                ),
            expand_topology: enabled_lanes.contains(&EvidenceFamily::TopologyWave)
                && retrieval_policy.cognitive_profile.requirements().topology,
            topology_rounds: retrieval_policy.topology_hops[index],
            topology_nodes: retrieval_policy.topology_states[index],
            resource_limit: retrieval_policy.resource_limits[index],
            materialize_evidence: query.result_need.need_evidence
                && matches!(
                    query.effort,
                    CognitiveEffort::Deep | CognitiveEffort::Maximum
                ),
            prefer_resource_synopsis: query.resources.synopsis_only
                || matches!(query.exploration, ExplorationIntent::Global),
        }
    }

    pub(super) fn allocation(total: usize, index: usize, branches: usize) -> usize {
        total / branches + usize::from(index < total % branches)
    }

    /// Leaf work consumes the operation budget; it cannot enlarge the closed
    /// lane set or replace the profile selected for the prepared query.
    pub(super) fn for_leaf(&self, bound: &mut BoundQuery, index: usize, count: usize) -> Self {
        let mut local = planned_profile_lanes(&bound.source_query, self.cognitive_profile);
        if !bound.source_query.is_exact_read()
            && bound.concept_enrichment != super::ConceptEnrichment::Off
        {
            local.push(EvidenceFamily::TagDirect);
            local.sort();
            local.dedup();
        }
        let mut plan = self.clone();
        plan.lane_budgets = self
            .lane_budgets
            .iter()
            .map(|(&family, &total)| {
                (
                    family,
                    if local.contains(&family) {
                        Self::allocation(total, index, count)
                    } else {
                        0
                    },
                )
            })
            .collect();
        plan.enabled_lanes = local
            .into_iter()
            .filter(|family| plan.lane_budget(*family) > 0)
            .collect();
        plan.candidate_limit = plan.lane_budgets.values().copied().max().unwrap_or(0);
        plan.final_validation_budget = Self::allocation(self.final_validation_budget, index, count);
        plan.topology_nodes = Self::allocation(self.topology_nodes, index, count);
        plan.resource_limit = Self::allocation(self.resource_limit, index, count);
        plan.expand_topology = plan.enabled_lanes.contains(&EvidenceFamily::TopologyWave);
        if bound.source_query.is_exact_read() {
            bound.concept_enrichment = super::ConceptEnrichment::Off;
            plan.concept_enrichment = super::ConceptEnrichment::Off;
        }
        bound.lane_budgets = plan.lane_budgets.clone();
        bound.enabled_lanes = plan.enabled_lanes.clone();
        plan
    }

    pub fn lane_budget(&self, family: EvidenceFamily) -> usize {
        self.lane_budgets.get(&family).copied().unwrap_or(0)
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
                        && self.cognitive_profile.requirements().query_embedding)),
            topology: self.expand_topology,
            concept: self.enabled_lanes.contains(&EvidenceFamily::TagDirect)
                || self.expand_topology,
            concept_vectors: query.capabilities.text_embedding != RequirementStrength::Forbidden
                && (self.concept_enrichment != super::ConceptEnrichment::Off
                    || (self.expand_topology
                        && self.cognitive_profile.requirements().query_embedding)),
        }
    }
}

fn effort_index(effort: CognitiveEffort) -> usize {
    match effort {
        CognitiveEffort::Light => 0,
        CognitiveEffort::Normal => 1,
        CognitiveEffort::Deep => 2,
        CognitiveEffort::Maximum => 3,
    }
}
