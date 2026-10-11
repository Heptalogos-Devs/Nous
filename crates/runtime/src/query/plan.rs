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
    pub sense_cues: bool,
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

    /// Retained for local plan tests and non-authority callers. Production
    /// query execution uses `BoundQuery::bind_query` and `for_bound_query`.
    pub fn for_query(query: &CognitiveQuery) -> Self {
        let policy = super::RetrievalPolicy::reference();
        let lanes = planned_profile_lanes(query, policy.cognitive_profile);
        let index = effort_index(query.effort);
        let multiplier = policy.effort_multipliers[index];
        let per_lane_max = policy.per_lane_max[index];
        let budget = query
            .result_need
            .limit
            .saturating_mul(multiplier)
            .clamp(policy.lane_min, per_lane_max);
        let budgets = lanes.iter().copied().map(|lane| (lane, budget)).collect();
        Self::from_parts(query, lanes, budgets, &policy)
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
            sense_cues: !query.is_exact_read()
                && query.capabilities.residual_sensing == RequirementStrength::Required,
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
                || self.expand_topology
                || self.sense_cues,
            concept_vectors: query.capabilities.text_embedding != RequirementStrength::Forbidden
                && (self.concept_enrichment != super::ConceptEnrichment::Off
                    || (self.expand_topology
                        && self.cognitive_profile.requirements().query_embedding)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn query(effort: CognitiveEffort) -> CognitiveQuery {
        CognitiveQuery {
            projection: Default::default(),
            temporal_frame: Default::default(),

            work_context: None,
            subject: SubjectId::new(),
            session: None,
            situation: Default::default(),
            expression: CognitiveQueryExpr {
                operation: QueryOperation::Atom,
                preferences: Vec::new(),
                children: Vec::new(),
                targets: Vec::new(),
                cues: vec![Cue::Text(TextCue {
                    text: "query".into(),
                })],
                constraints: Default::default(),
            },
            exploration: ExplorationIntent::None,
            resources: Default::default(),
            result_need: Default::default(),
            effort,
            capabilities: Default::default(),
            diagnostics: DiagnosticsRequest::Summary,
        }
    }

    #[test]
    fn deep_text_does_not_enable_topology_without_explicit_intent() {
        assert!(!QueryPlan::for_query(&query(CognitiveEffort::Deep)).expand_topology);
    }

    #[test]
    fn validation_budget_is_the_frozen_reference_formula() {
        let mut value = query(CognitiveEffort::Normal);
        value.result_need.limit = 12;
        assert_eq!(QueryPlan::for_query(&value).final_validation_budget, 48);
    }

    #[test]
    fn exact_read_is_scoped_and_does_not_require_serving() {
        let mut value = query(CognitiveEffort::Deep);
        value.expression.targets.push(QueryTarget::Exact {
            reference: CognitiveRef::Memory(MemoryId::new()),
        });
        value.session = Some(SessionId::new());
        value.exploration = ExplorationIntent::BoundedAssociative;
        value.capabilities.residual_sensing = RequirementStrength::Required;
        let plan = QueryPlan::for_query(&value);
        assert_eq!(plan.enabled_lanes, vec![EvidenceFamily::Exact]);
        assert!(!plan.expand_topology);
        assert!(!plan.sense_cues);
        assert_eq!(plan.serving_need(&value), ServingNeed::default());
        let atom = value.expression.clone();
        value.expression.operation = QueryOperation::Any;
        value.expression.cues.clear();
        value.expression.children = vec![atom.clone(), atom.clone()];
        // Empty child targets inherit the parent's exact target.
        value.expression.children[0].targets.clear();
        assert!(value.is_exact_read());
        assert_eq!(
            QueryPlan::for_query(&value).enabled_lanes,
            vec![EvidenceFamily::Exact]
        );
        value.expression.targets.clear();
        assert!(!value.is_exact_read());
        let mixed = QueryPlan::for_query(&value);
        assert!(mixed.enabled_lanes.contains(&EvidenceFamily::Exact));
        assert!(mixed.enabled_lanes.contains(&EvidenceFamily::Lexical));
    }
}
