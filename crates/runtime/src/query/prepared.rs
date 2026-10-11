// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{BoundQuery, CognitiveContributors, QueryExecution, QueryPlan};
use crate::CognitiveRuntimeService;
use nous_core::*;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use tokio::sync::OwnedSemaphorePermit;

#[derive(Debug, Clone, Copy)]
pub struct QueryHostRole {
    pub configured: bool,
    pub requirement: RequirementStrength,
}
#[derive(Debug, Clone, Copy)]
pub struct QueryHostPlan {
    pub embedding: QueryHostRole,
    pub concept_enrichment: QueryHostRole,
    pub rerank: QueryHostRole,
    pub rerank_candidate_limit: usize,
}
#[derive(Debug, Clone, Copy)]
pub struct EffectiveQueryHostRole {
    pub configured: bool,
    pub requirement: RequirementStrength,
}
#[derive(Debug, Clone, Copy)]
pub struct QueryRunHostPolicy {
    pub embedding: EffectiveQueryHostRole,
    pub concept_enrichment: EffectiveQueryHostRole,
    pub rerank: EffectiveQueryHostRole,
    pub rerank_candidate_limit: usize,
}
impl QueryRunHostPolicy {
    pub fn for_query(query: &CognitiveQuery, host: QueryHostPlan) -> Result<Self> {
        fn effective(
            requested: RequirementStrength,
            role: QueryHostRole,
        ) -> EffectiveQueryHostRole {
            let requirement = if requested == RequirementStrength::Forbidden
                || role.requirement == RequirementStrength::Forbidden
            {
                RequirementStrength::Forbidden
            } else if requested == RequirementStrength::Required
                || role.requirement == RequirementStrength::Required
            {
                RequirementStrength::Required
            } else {
                RequirementStrength::Optional
            };
            EffectiveQueryHostRole {
                configured: role.configured,
                requirement,
            }
        }
        let rerank_candidate_limit = if host.rerank_candidate_limit == 0 {
            64
        } else {
            host.rerank_candidate_limit
        };
        if !(2..=64).contains(&rerank_candidate_limit) {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "Invalid query rerank candidate limit",
            ));
        }
        Ok(Self {
            embedding: effective(query.capabilities.text_embedding, host.embedding),
            concept_enrichment: effective(
                query.capabilities.query_concept_enrichment,
                host.concept_enrichment,
            ),
            rerank: effective(
                if query.is_exact_read() {
                    RequirementStrength::Forbidden
                } else {
                    query.capabilities.rerank
                },
                host.rerank,
            ),
            rerank_candidate_limit,
        })
    }
}

pub struct QueryRunContext {
    plan: QueryPlan,
    host_policy: QueryRunHostPolicy,
    _slot: OwnedSemaphorePermit,
}

/// The phase determines which payload exists while a single Core↔Kernel stream is open.
// Each stage owns one query; boxing only Finalize would not shrink the BoundQuery stages.
#[allow(clippy::large_enum_variant)]
pub enum QueryRun {
    Embedding {
        bound: BoundQuery,
        context: QueryRunContext,
    },
    Concept {
        bound: BoundQuery,
        context: QueryRunContext,
    },
    Finalize {
        execution: QueryExecution,
        context: QueryRunContext,
    },
}

impl QueryRunContext {
    pub fn new(
        plan: QueryPlan,
        host_policy: QueryRunHostPolicy,
        slot: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            plan,
            host_policy,
            _slot: slot,
        }
    }
    pub fn plan(&self) -> &QueryPlan {
        &self.plan
    }
    pub fn host_policy(&self) -> QueryRunHostPolicy {
        self.host_policy
    }
}

impl QueryRun {
    pub fn embedding(bound: BoundQuery, context: QueryRunContext) -> Self {
        Self::Embedding { bound, context }
    }
    pub fn concept(bound: BoundQuery, context: QueryRunContext) -> Self {
        Self::Concept { bound, context }
    }
    pub fn finalize(execution: QueryExecution, context: QueryRunContext) -> Self {
        Self::Finalize { execution, context }
    }
    pub fn bound(&self) -> &BoundQuery {
        match self {
            Self::Embedding { bound, .. } | Self::Concept { bound, .. } => bound,
            Self::Finalize { execution, .. } => &execution.bound,
        }
    }
    pub fn bound_mut(&mut self) -> &mut BoundQuery {
        match self {
            Self::Embedding { bound, .. } | Self::Concept { bound, .. } => bound,
            Self::Finalize { execution, .. } => &mut execution.bound,
        }
    }
    pub fn plan(&self) -> &QueryPlan {
        match self {
            Self::Embedding { context, .. }
            | Self::Concept { context, .. }
            | Self::Finalize { context, .. } => context.plan(),
        }
    }
    pub fn host_policy(&self) -> QueryRunHostPolicy {
        match self {
            Self::Embedding { context, .. }
            | Self::Concept { context, .. }
            | Self::Finalize { context, .. } => context.host_policy(),
        }
    }
    pub fn into_finalized(self) -> Result<(QueryExecution, QueryRunContext)> {
        match self {
            Self::Finalize { execution, context } => Ok((execution, context)),
            Self::Embedding { .. } | Self::Concept { .. } => Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::FailedPrecondition,
                "Query result is unavailable",
            )),
        }
    }
}

impl CognitiveRuntimeService {
    pub async fn ensure_read_context_current(&self, context: &super::ReadContext) -> Result<()> {
        let current_epoch = self.store.serving_purge_epoch(context.subject).await?;
        if current_epoch != context.purge_epoch {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::StaleContext,
                "Run context is stale",
            )
            .with_context("reason", "purge_epoch_changed"));
        }
        Ok(())
    }

    pub fn acquire_query_slot(&self, limit: usize) -> Result<OwnedSemaphorePermit> {
        const MAX_QUERY_SLOTS: usize = 64;
        if !(1..=MAX_QUERY_SLOTS).contains(&limit) {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "Invalid query slot limit",
            ));
        }
        let _admission = self.query_slot_admission.lock().map_err(|_| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InternalFailure,
                "Query capacity state is unavailable",
            )
        })?;
        let active = MAX_QUERY_SLOTS - self.query_slots.available_permits();
        if active >= limit {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::CapacityExhausted,
                "Query capacity is exhausted",
            )
            .with_context("reason", "query_capacity"));
        }
        self.query_slots.clone().try_acquire_owned().map_err(|_| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::CapacityExhausted,
                "Query capacity is exhausted",
            )
            .with_context("reason", "query_capacity")
        })
    }

    pub fn validate_query_execution_capacity(&self, execution: &QueryExecution) -> Result<()> {
        let bytes = execution
            .result
            .results
            .iter()
            .map(|hit| hit.representation.as_ref().map_or(0, String::len))
            .sum::<usize>();
        if execution.result.results.len() > 64 || bytes > 2 * 1024 * 1024 {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::CapacityExhausted,
                "Query continuation exceeds its bounded result capacity",
            )
            .with_context("reason", "query_capacity"));
        }
        Ok(())
    }

    pub async fn finish_query_run(
        &self,
        run: QueryRun,
        order: Vec<(CognitiveRef, f64)>,
        external_results: Vec<ExternalResourceResult>,
        contributors: CognitiveContributors<'_>,
    ) -> Result<CognitiveQueryResult> {
        let (execution, _context) = run.into_finalized()?;
        let subject = execution.bound.read_context.subject;
        self.ensure_read_context_current(&execution.bound.read_context)
            .await?;
        if order.len() > 64 || order.iter().any(|(_, score)| !score.is_finite()) {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "Rerank mapping is invalid",
            ));
        }
        let pool: HashSet<_> = execution
            .result
            .results
            .iter()
            .map(|hit| hit.reference.clone())
            .collect();
        let requested: HashSet<_> = order
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect();
        if requested.len() != order.len() || !requested.is_subset(&pool) {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "Rerank mapping contains unknown candidates",
            ));
        }
        let leaf_count = execution
            .bound
            .source_query
            .scopes()
            .iter()
            .filter(|scope| scope.operation == QueryOperation::Atom)
            .count();
        let mut outputs = vec![Vec::new(); leaf_count];
        for leaf in &execution.leaves {
            let mut fresh = HashMap::new();
            for owner in [contributors.memory, contributors.material]
                .into_iter()
                .flatten()
            {
                let references = leaf
                    .hits
                    .iter()
                    .filter(|hit| owner.owns(&hit.reference))
                    .map(|hit| hit.reference.clone())
                    .collect::<Vec<_>>();
                let (hits, _) = owner
                    .validate_and_materialize(subject, &references, &leaf.bound)
                    .await?;
                fresh.extend(hits.into_iter().map(|hit| (hit.reference.clone(), hit)));
            }
            for hit in &leaf.hits {
                if !pool.contains(&hit.reference) {
                    continue;
                }
                let valid = if [contributors.memory, contributors.material]
                    .into_iter()
                    .flatten()
                    .any(|owner| owner.owns(&hit.reference))
                {
                    same_stamp(fresh.get(&hit.reference), hit)
                } else {
                    self.generic_still_valid(subject, &leaf.bound, &hit.reference)
                        .await?
                };
                if !valid {
                    continue;
                }
                let Some(original) = execution
                    .result
                    .results
                    .iter()
                    .find(|original| original.reference == hit.reference)
                else {
                    continue;
                };
                outputs[leaf.ordinal].push(original.clone());
            }
        }
        let valid = super::tree::combine(
            &execution.bound.source_query.expression,
            &mut outputs.into_iter(),
        );
        let mut result = execution.result;
        let before = result.results.len();
        result
            .results
            .retain(|hit| valid.contains_key(&hit.reference));
        let dropped = before - result.results.len();
        if dropped > 0 {
            result.status = QueryStatus::Partial;
            result.degradation.push(Degradation {
                code: "authority_changed_during_rerank".into(),
                detail: Some(format!(
                    "discarded {dropped} candidates along the original BoundQuery"
                )),
            });
            if let Some(diagnostics) = &mut result.diagnostics {
                diagnostics
                    .candidate_counts
                    .insert("drop_authority_changed_during_rerank".into(), dropped);
            }
        }
        if !order.is_empty() {
            apply_model_order(
                &mut result.results,
                order,
                execution.bound.retrieval_policy.rrf_k,
            );
        }
        result = finalize_query_response(&execution.bound, result);
        self.finalize_resources(subject, &mut result, external_results)
            .await?;
        self.record_finalized_query_feedback(&execution.bound, &result)
            .await?;
        Ok(result)
    }

    /// Complete a direct one-shot query execution at the same response fence.
    pub fn complete_query_execution<'a>(
        &'a self,
        execution: QueryExecution,
    ) -> Pin<Box<dyn Future<Output = Result<QueryExecution>> + Send + 'a>> {
        Box::pin(async move {
            let QueryExecution {
                read_lease,
                result,
                bound,
                leaves,
            } = execution;
            if !result.resource_actions.is_empty() {
                return Err(Error::new(
                    ErrorOwner::Runtime,
                    ErrorCode::FailedPrecondition,
                    "Query resources must be finalized through QueryRun",
                ));
            }
            let result = finalize_query_response(&bound, result);
            self.record_finalized_query_feedback(&bound, &result)
                .await?;
            Ok(QueryExecution {
                read_lease,
                result,
                bound,
                leaves,
            })
        })
    }

    async fn record_finalized_query_feedback(
        &self,
        bound: &BoundQuery,
        result: &CognitiveQueryResult,
    ) -> Result<()> {
        self.ensure_read_context_current(&bound.read_context)
            .await?;
        self.record_query_feedback(bound, result).await?;
        self.ensure_read_context_current(&bound.read_context)
            .await?;
        Ok(())
    }

    async fn generic_still_valid(
        &self,
        subject: SubjectId,
        bound: &super::BoundQuery,
        reference: &CognitiveRef,
    ) -> Result<bool> {
        if let Some(binding) = bound
            .exact_bindings
            .iter()
            .find(|binding| binding.bound_ref == *reference)
            && binding.mutable_object
        {
            match self
                .store
                .bind_exact_reference(subject, &binding.requested_ref)
                .await
            {
                Ok((current, epoch, _))
                    if current == binding.bound_ref && epoch == binding.bound_object_epoch => {}
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::NotFound
                            | ErrorCode::UnknownReference
                            | ErrorCode::InvalidRequest
                            | ErrorCode::ReferenceTypeMismatch
                    ) =>
                {
                    return Ok(false);
                }
                Err(error) => return Err(error),
            }
        }
        match self.store.validate_reference(subject, reference).await {
            Ok(()) => Ok(super::orchestrate::generic_constraints_match(
                &bound.source_query.expression.constraints,
            )),
            Err(error)
                if matches!(
                    error.code,
                    ErrorCode::NotFound
                        | ErrorCode::UnknownReference
                        | ErrorCode::InvalidRequest
                        | ErrorCode::ReferenceTypeMismatch
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

fn same_stamp(current: Option<&CognitiveHit>, old: &super::types::CandidateStamp) -> bool {
    current.is_some_and(|current| {
        current.authority_epoch == old.authority_epoch && current.revision == old.revision
    })
}

fn finalize_query_response(
    bound: &BoundQuery,
    mut result: CognitiveQueryResult,
) -> CognitiveQueryResult {
    result
        .results
        .truncate(bound.source_query.result_need.limit);
    for (rank, hit) in result.results.iter_mut().enumerate() {
        hit.match_evidence.final_rank = (rank + 1) as u32;
    }
    result
}

fn apply_model_order(hits: &mut Vec<CognitiveHit>, mut order: Vec<(CognitiveRef, f64)>, k: f64) {
    if order.is_empty() {
        return;
    }
    order.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut remaining: HashMap<_, _> = hits
        .drain(..)
        .map(|hit| (hit.reference.clone(), hit))
        .collect();
    for (reference, score) in order {
        if let Some(mut hit) = remaining.remove(&reference) {
            hit.match_evidence.rerank_score = Some(score);
            hit.match_evidence
                .families
                .push(EvidenceFamily::LanguageRerank);
            hits.push(hit);
        }
    }
    let mut tail: Vec<_> = remaining.into_values().collect();
    tail.sort_by_key(|hit| hit.match_evidence.final_rank);
    hits.extend(tail);
    for (rank, hit) in hits.iter_mut().enumerate() {
        hit.match_evidence.final_score = (k + 1.0) / (k + (rank + 1) as f64);
    }
}
