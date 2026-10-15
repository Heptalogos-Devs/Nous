// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{
    CandidateRankInput, LaneCandidate, LaneOutput, LaneStatus, rank_candidates_with_policy,
};
use crate::{BoundQuery, CognitiveRuntimeService, QueryPlan};
use nous_core::*;
use std::collections::{BTreeMap, HashMap};

#[async_trait::async_trait]
pub trait CognitiveContributor: Send + Sync {
    fn owns(&self, reference: &CognitiveRef) -> bool;

    async fn direct_lanes(&self, bound: &BoundQuery, plan: &QueryPlan) -> Result<Vec<LaneOutput>>;

    async fn validate_and_materialize(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        bound: &BoundQuery,
    ) -> Result<(Vec<CognitiveHit>, BTreeMap<String, usize>)>;
}

/// Shared Serving owns projection-backed lexical/dense/topology lanes. Domain
/// owners receive the resulting references and only validate/materialize their
/// own Authority objects after the runtime performs one fusion.
#[async_trait::async_trait]
pub trait SharedLaneProvider: Send + Sync {
    async fn activate(&self, bound: &BoundQuery) -> Result<super::QueryActivation> {
        Ok(bound.activation.clone())
    }
    async fn lanes(&self, bound: &BoundQuery, plan: &QueryPlan) -> Result<Vec<LaneOutput>>;
}

/// The current fixed cognition-owner set. Adding an owner is a compile-time
/// composition change; callers do not discover domains dynamically.
pub struct CognitiveContributors<'a> {
    pub shared: Option<&'a dyn SharedLaneProvider>,
    pub owners: Vec<&'a dyn CognitiveContributor>,
}

impl CognitiveRuntimeService {
    pub async fn execute_query_with_plan(
        &self,
        bound: BoundQuery,
        contributors: CognitiveContributors<'_>,
        plan: QueryPlan,
        validated_pool_limit: Option<usize>,
    ) -> Result<super::QueryExecution> {
        let mut bound = bound;
        if bound.historical_authority.is_none()
            && (bound.source_query.temporal_frame.authority_view != AuthorityView::Current
                || bound.source_query.temporal_frame.revision_view != RevisionView::Current)
        {
            return Err(Error::Unavailable(
                "historical query requires its owner-projected Authority and Serving view".into(),
            ));
        }
        if let Some(shared) = contributors
            .shared
            .filter(|_| !bound.source_query.is_exact_read())
        {
            bound.activation = shared.activate(&bound).await?;
        }
        if bound.concept_enrichment == super::ConceptEnrichment::Model
            && bound.source_query.capabilities.query_concept_enrichment
                != RequirementStrength::Forbidden
            && !bound.activation.model_completed
        {
            if bound.source_query.capabilities.query_concept_enrichment
                == RequirementStrength::Required
            {
                return Err(Error::Unavailable(
                    "required query concept model unavailable".into(),
                ));
            }
            if !bound
                .activation
                .degradation
                .iter()
                .any(|d| d.code == "query_concept_model_unavailable")
            {
                bound.activation.degradation.push(Degradation {
                    code: "query_concept_model_unavailable".into(),
                    detail: Some("Host query concept model result unavailable".into()),
                });
            }
        }
        bound.activation.frozen = true;
        if validated_pool_limit.is_some_and(|limit| limit == 0 || limit > 64) {
            return Err(Error::Invalid("validated pool limit must be 1..64".into()));
        }
        let output_limit = validated_pool_limit.unwrap_or(bound.source_query.result_need.limit);
        let execution = if bound.source_query.expression.operation != QueryOperation::Atom {
            self.query_tree(bound, contributors, plan, output_limit)
                .await?
        } else {
            self.query_leaf_execution(bound, &contributors, plan, output_limit)
                .await?
        };
        if validated_pool_limit.is_none() && execution.result.resource_actions.is_empty() {
            self.record_query_feedback(&execution.bound, &execution.result)
                .await?;
        }
        Ok(execution)
    }
    async fn query_leaf_execution(
        &self,
        bound: BoundQuery,
        contributors: &CognitiveContributors<'_>,
        plan: QueryPlan,
        output_limit: usize,
    ) -> Result<super::QueryExecution> {
        let result = self
            .query_atom_with_plan(bound.clone(), contributors, plan, output_limit)
            .await?;
        let leaves = vec![super::types::BoundLeaf {
            ordinal: 0,
            expression: bound.source_query.expression.clone(),
            exact_bindings: bound.exact_bindings.clone(),
            hits: result
                .results
                .iter()
                .map(super::types::CandidateStamp::from)
                .collect(),
        }];
        Ok(super::QueryExecution {
            bound,
            leaves,
            result,
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "query orchestration keeps single fusion, owner batching, and final ordering in one semantic boundary"
    )]
    pub(super) async fn query_atom_with_plan(
        &self,
        bound: BoundQuery,
        contributors: &CognitiveContributors<'_>,
        plan: QueryPlan,
        output_limit: usize,
    ) -> Result<CognitiveQueryResult> {
        let query = &bound.source_query;
        let mut result = CognitiveQueryResult {
            query_id: bound.query_id,
            generation: QueryGenerationTrace::default(),
            status: QueryStatus::Complete,
            results: Vec::new(),
            resource_actions: Vec::new(),
            resource_records: Vec::new(),
            degradation: Vec::new(),
            diagnostics: None,
        };
        let exact_read = query.is_exact_read();
        result
            .degradation
            .extend(bound.activation.degradation.clone());
        let mut lane_outputs = Vec::new();
        if let Some(shared) = contributors.shared.filter(|_| !exact_read) {
            lane_outputs.extend(shared.lanes(&bound, &plan).await?);
        }
        if !exact_read {
            for owner in &contributors.owners {
                lane_outputs.extend(owner.direct_lanes(&bound, &plan).await?);
            }
        }
        let mut exact_output = LaneOutput::empty(EvidenceFamily::Exact, LaneStatus::Ready);
        for binding in &bound.exact_bindings {
            exact_output.candidates.push(LaneCandidate {
                reference: binding.bound_ref.clone(),
                rank: 1,
                variants: Vec::new(),
                provider_metadata: serde_json::Value::Null,
            });
        }
        if !exact_output.candidates.is_empty() {
            lane_outputs.push(exact_output);
        }
        let runtime_allowed = query.projection.has_cognition()
            || query.projection.domains.contains(&ResultDomain::Evidence);
        if runtime_allowed && bound.lane_enabled(EvidenceFamily::Runtime) {
            for source in bound.runtime_sources() {
                lane_outputs.push(LaneOutput {
                    family: EvidenceFamily::Runtime,
                    status: LaneStatus::Ready,
                    generation_ref: None,
                    authority_watermark: None,
                    candidates: vec![LaneCandidate {
                        reference: source.reference.clone(),
                        rank: 1,
                        variants: vec![source.origin.clone()],
                        provider_metadata: serde_json::Value::Null,
                    }],
                    diagnostics: Vec::new(),
                    topology_work: None,
                });
            }
        }
        for output in &mut lane_outputs {
            output.candidates.truncate(plan.lane_budget(output.family));
        }
        let mut candidates = HashMap::<CognitiveRef, CandidateRankInput>::new();
        let mut readouts = HashMap::<CognitiveRef, BTreeMap<String, serde_json::Value>>::new();
        let mut lane_drops = BTreeMap::new();
        for output in &lane_outputs {
            if matches!(output.status, LaneStatus::Unavailable | LaneStatus::Stale) {
                result.degradation.push(Degradation {
                    code: format!("{:?}_lane_unavailable", output.family).to_lowercase(),
                    detail: output.diagnostics.first().cloned(),
                });
            }
            if output.status == LaneStatus::Truncated {
                result.status = QueryStatus::Partial;
                result.degradation.push(Degradation {
                    code: format!("{:?}_lane_truncated", output.family).to_lowercase(),
                    detail: Some("Configured lane work budget exhausted".into()),
                });
            }
            for candidate in &output.candidates {
                if !candidate.provider_metadata.is_null() {
                    readouts
                        .entry(candidate.reference.clone())
                        .or_default()
                        .insert(
                            format!("{:?}", output.family).to_lowercase(),
                            candidate.provider_metadata.clone(),
                        );
                }
                let entry = candidates
                    .entry(candidate.reference.clone())
                    .or_insert_with(|| CandidateRankInput {
                        reference: candidate.reference.clone(),
                        family_ranks: HashMap::new(),
                        variants: Vec::new(),
                    });
                entry
                    .family_ranks
                    .entry(output.family)
                    .and_modify(|rank| *rank = (*rank).min(candidate.rank as usize))
                    .or_insert(candidate.rank as usize);
                entry.variants.extend(candidate.variants.clone());
            }
        }
        let before = candidates.len();
        candidates.retain(|reference, _| query.projection.allows_reference(reference));
        let dropped = before - candidates.len();
        if dropped > 0 {
            *lane_drops.entry("domain_ineligible".into()).or_default() += dropped;
        }
        let rank_inputs = candidates.into_values().collect::<Vec<_>>();
        let ranked = rank_candidates_with_policy(
            &rank_inputs,
            &bound.enabled_lanes,
            &bound.retrieval_policy,
        );
        let validation_bound = plan.final_validation_budget.min(ranked.len());
        let validation_budget_exhausted = ranked.len() > validation_bound;
        if validation_budget_exhausted {
            result.status = QueryStatus::Partial;
            result.degradation.push(Degradation {
                code: "validation_budget_exhausted".into(),
                detail: Some(format!(
                    "validated {validation_bound} of {} ranked candidates",
                    ranked.len()
                )),
            });
        }
        let owners = &contributors.owners;
        let mut by_owner = vec![Vec::<CognitiveRef>::new(); owners.len()];
        let mut generic = Vec::new();
        for candidate in ranked.iter().take(validation_bound) {
            let matching = owners
                .iter()
                .enumerate()
                .filter(|(_, owner)| owner.owns(&candidate.reference))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matching.len() > 1 {
                return Err(Error::Infrastructure(format!(
                    "multiple owners claim {}",
                    candidate.reference
                )));
            }
            if let Some(index) = matching.first() {
                by_owner[*index].push(candidate.reference.clone());
            } else if is_persistent_cognition(&candidate.reference)
                || matches!(
                    candidate.reference,
                    CognitiveRef::Artifact(_)
                        | CognitiveRef::Occurrence(_)
                        | CognitiveRef::SourceRegion(_)
                        | CognitiveRef::DerivedRepresentation(_)
                        | CognitiveRef::DerivedRegion(_)
                )
            {
                return Err(Error::Unavailable(format!(
                    "no semantic owner available for {}",
                    candidate.reference
                )));
            } else {
                generic.push(candidate.reference.clone());
            }
        }
        let mut materialized = HashMap::new();
        let mut validation_drops = BTreeMap::new();
        merge_counts(&mut validation_drops, lane_drops);
        if validation_budget_exhausted {
            validation_drops.insert(
                "validation_budget_exhausted".into(),
                ranked.len().saturating_sub(validation_bound),
            );
        }
        for (index, owner) in owners.iter().enumerate() {
            if !by_owner[index].is_empty() {
                let (hits, drops) = owner
                    .validate_and_materialize(query.subject, &by_owner[index], &bound)
                    .await?;
                for hit in hits {
                    materialized.insert(hit.reference.clone(), hit);
                }
                merge_counts(&mut validation_drops, drops);
            }
        }
        for reference in generic {
            if let Some(reason) = self.generic_rejection(&bound, &reference).await? {
                *validation_drops.entry(reason.into()).or_default() += 1;
                continue;
            }
            let hit = reference_hit(reference.clone(), EvidenceFamily::Exact, query);
            materialized.insert(reference, hit);
        }
        for candidate in ranked.into_iter().take(validation_bound) {
            if let Some(mut hit) = materialized.remove(&candidate.reference) {
                hit.match_evidence = MatchEvidence {
                    preference_score: 0.0,
                    rerank_score: None,
                    baseline_rank: 0,
                    final_rank: 0,
                    families: candidate.families.clone(),
                    base_rank_score: candidate.baseline_score,
                    best_lane_rank: candidate.best_lane_rank as u32,
                    enabled_lane_count: candidate.families.len() as u32,
                    final_score: candidate.final_score,
                    variants: candidate.variants,
                    explanation: readouts
                        .get(&candidate.reference)
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|error| Error::Infrastructure(error.to_string()))?,
                };
                result.results.push(hit);
            }
        }
        let (actions, degradation) = self.resource_actions_for_query(query, &plan).await?;
        result.resource_actions = actions;
        result.degradation.extend(degradation);
        super::preferences::apply(
            &mut result.results,
            &query.expression.preferences,
            bound.bound_at,
            &bound.retrieval_policy,
        );
        result.results.truncate(output_limit);
        if !result.degradation.is_empty() && result.status != QueryStatus::Partial {
            result.status = if result
                .degradation
                .iter()
                .any(|d| d.code == "required_external_authority_unresolved")
            {
                QueryStatus::Partial
            } else {
                QueryStatus::Degraded
            };
        }
        if validation_budget_exhausted {
            result.status = QueryStatus::Partial;
        }
        explain_plan(&mut result, query, &plan);
        if let Some(diagnostics) = result.diagnostics.as_mut() {
            if query.diagnostics == DiagnosticsRequest::Full {
                diagnostics.trace = Some(serde_json::json!({
                    "query_activation": bound.activation,
                    "readouts": result.results.iter().filter_map(|hit| readouts.get(&hit.reference)
                        .map(|value| serde_json::json!({"reference": hit.reference, "lanes": value})))
                        .collect::<Vec<_>>()
                }));
            }
            for lane in &lane_outputs {
                let name = format!("{:?}", lane.family).to_lowercase();
                diagnostics
                    .lane_status
                    .insert(name.clone(), format!("{:?}", lane.status).to_lowercase());
                *diagnostics
                    .candidate_counts
                    .entry(format!("{name}_candidates"))
                    .or_default() += lane.candidates.len();
                if let Some(work) = &lane.topology_work {
                    diagnostics
                        .lane_status
                        .insert("topology_profile".into(), work.profile_id.clone());
                    diagnostics.lane_status.insert(
                        "topology_profile_digest".into(),
                        work.profile_digest.clone(),
                    );
                    diagnostics
                        .candidate_counts
                        .insert("topology_activated_edges".into(), work.activated_edges);
                    diagnostics
                        .candidate_counts
                        .insert("topology_max_hop_observed".into(), work.max_hop_observed);
                    diagnostics
                        .lane_status
                        .insert("topology_mechanism".into(), work.mechanism.clone());
                    diagnostics
                        .candidate_counts
                        .insert("topology_seed_count".into(), work.seed_count);
                    diagnostics
                        .candidate_counts
                        .insert("topology_visited_nodes".into(), work.visited_nodes);
                    diagnostics.topology_complete = Some(work.complete);
                    diagnostics.topology_discarded_mass = work.discarded_mass;
                }
            }
        }
        if !validation_drops.is_empty() {
            let diagnostics = result.diagnostics.get_or_insert(QueryDiagnostics {
                candidate_counts: BTreeMap::new(),
                lane_status: BTreeMap::new(),
                topology_complete: None,
                topology_discarded_mass: None,
                trace: None,
            });
            for (reason, count) in validation_drops {
                diagnostics
                    .candidate_counts
                    .insert(format!("drop_{reason}"), count);
            }
        }
        if validation_budget_exhausted {
            result.status = QueryStatus::Partial;
        }
        Ok(result)
    }
}

fn merge_counts(target: &mut BTreeMap<String, usize>, incoming: BTreeMap<String, usize>) {
    for (key, value) in incoming {
        *target.entry(key).or_default() += value;
    }
}

fn is_persistent_cognition(reference: &CognitiveRef) -> bool {
    matches!(
        reference.domain(),
        CognitiveDomain::Memory | CognitiveDomain::SelfCognition
    ) && !matches!(reference, CognitiveRef::Tag(_))
}

fn explain_plan(result: &mut CognitiveQueryResult, query: &CognitiveQuery, plan: &QueryPlan) {
    if query.diagnostics != DiagnosticsRequest::None {
        let diagnostics = result.diagnostics.get_or_insert(QueryDiagnostics {
            candidate_counts: BTreeMap::new(),
            lane_status: BTreeMap::new(),
            topology_complete: None,
            topology_discarded_mass: None,
            trace: None,
        });
        diagnostics.lane_status.insert(
            "effort".into(),
            format!("{:?}", query.effort).to_lowercase(),
        );
        diagnostics.lane_status.insert(
            "topology_plan".into(),
            if plan.expand_topology {
                "enabled"
            } else {
                "skipped"
            }
            .into(),
        );
        diagnostics
            .candidate_counts
            .insert("planned_candidate_bound".into(), plan.candidate_limit);
        diagnostics.lane_status.insert(
            "resource_plan".into(),
            format!(
                "limit={},synopsis_preferred={}",
                plan.resource_limit, plan.prefer_resource_synopsis
            ),
        );
    }
}

fn reference_hit(
    reference: CognitiveRef,
    family: EvidenceFamily,
    query: &CognitiveQuery,
) -> CognitiveHit {
    let authority = match reference {
        CognitiveRef::Memory(_)
        | CognitiveRef::MemoryRevision(_)
        | CognitiveRef::Episode(_)
        | CognitiveRef::EpisodeRevision(_)
        | CognitiveRef::Journal(_)
        | CognitiveRef::JournalRevision(_)
        | CognitiveRef::Tag(_)
        | CognitiveRef::CognitiveSchema(_)
        | CognitiveRef::CognitiveSchemaRevision(_)
        | CognitiveRef::CognitiveSeedVersion(_) => AuthorityClass::SubjectCognition,
        CognitiveRef::Resource(_) => AuthorityClass::ResourceDescriptor,
        CognitiveRef::DerivedRepresentation(_) | CognitiveRef::DerivedRegion(_) => {
            AuthorityClass::Interpretation
        }
        _ => AuthorityClass::Evidence,
    };
    CognitiveHit {
        authority_epoch: None,
        preference_refs: Vec::new(),
        reference: reference.clone(),
        revision: None,
        semantic_role: Some("reference".into()),
        cognitive_role: None,
        formation_mode: None,
        representation: None,
        authority,
        freshness: FreshnessDescriptor {
            occurred: Vec::new(),
            observed_at: None,
            valid_time: TemporalExtent::Unknown,
            formed_at: None,
            recorded_at: None,
        },
        entity_refs: Vec::new(),
        evidence: if query.result_need.need_evidence {
            vec![EvidenceHandle {
                epistemic_relation: None,
                reference: reference.clone(),
                basis_role: "source".into(),
            }]
        } else {
            Vec::new()
        },
        match_evidence: MatchEvidence {
            preference_score: 0.0,
            rerank_score: None,
            baseline_rank: 0,
            final_rank: 0,
            families: vec![family],
            base_rank_score: 1.0,
            best_lane_rank: 1,
            enabled_lane_count: 1,
            final_score: 1.0,
            variants: Vec::new(),
            explanation: None,
        },
        materialization: if query.result_need.need_materialization_handles {
            vec![MaterializationHandle {
                reference,
                level: "source".into(),
            }]
        } else {
            Vec::new()
        },
    }
}

// Runtime/host references carry no Material time or source claims.
pub(super) fn generic_constraints_match(c: &QueryConstraints) -> bool {
    c.occurred.is_none()
        && c.observed.is_none()
        && c.valid.is_none()
        && c.formed.is_none()
        && c.recorded.is_none()
        && c.source_classes_include.is_empty()
        && c.modalities.is_empty()
        && c.evidence_classes.is_empty()
        && c.cognitive_roles_include.is_empty()
        && c.formation_modes_include.is_empty()
        && c.entity_requirements.is_empty()
        && c.authority.is_none()
}
