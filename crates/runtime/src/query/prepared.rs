// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{BoundQuery, CognitiveContributors, QueryExecution, QueryPlan};
use crate::CognitiveRuntimeService;
use nous_core::*;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Infrastructure permission for one query opportunity, separate from its immutable semantics.
#[derive(Debug, Clone, Copy)]
pub struct QueryLease {
    deadline: Instant,
}
impl QueryLease {
    pub fn new(duration: Duration) -> Result<Self> {
        if duration < Duration::from_millis(1) || duration > Duration::from_secs(3600) {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "Invalid query lease duration",
            ));
        }
        Ok(Self {
            deadline: Instant::now() + duration,
        })
    }
    pub fn require_live(self) -> Result<()> {
        if self.live() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::Unavailable,
                "Query execution opportunity expired",
            ))
        }
    }
    fn live(self) -> bool {
        Instant::now() < self.deadline
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryRunStage {
    Embedding,
    Concept,
    Finalize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryRunHandle {
    pub query_run_id: Uuid,
    pub revision: u64,
}

/// One request-scoped continuation, retained only while Core must perform a
/// real model or resource action. A revision is consumed exactly once.
pub struct QueryRun {
    id: Uuid,
    subject: SubjectId,
    revision: u64,
    stage: QueryRunStage,
    lease: QueryLease,
    bound: BoundQuery,
    plan: QueryPlan,
    execution: Option<QueryExecution>,
    pub host_policy: QueryRunHostPolicy,
}
impl QueryRun {
    pub fn new(
        bound: BoundQuery,
        lease: QueryLease,
        host_policy: QueryRunHostPolicy,
        stage: QueryRunStage,
        plan: QueryPlan,
    ) -> Result<Self> {
        lease.require_live()?;
        let id = bound.query_id;
        let subject = bound.read_context.subject;
        Ok(Self {
            id,
            subject,
            revision: 0,
            stage,
            lease,
            bound,
            plan,
            execution: None,
            host_policy,
        })
    }
    pub fn handle(&self) -> QueryRunHandle {
        QueryRunHandle {
            query_run_id: self.id,
            revision: self.revision,
        }
    }
    pub fn stage(&self) -> QueryRunStage {
        self.stage
    }
    pub fn subject(&self) -> SubjectId {
        self.subject
    }
    pub fn lease(&self) -> QueryLease {
        self.lease
    }
    pub fn plan(&self) -> &QueryPlan {
        &self.plan
    }
    pub fn bound(&self) -> &BoundQuery {
        self.execution
            .as_ref()
            .map_or(&self.bound, |execution| &execution.bound)
    }
    pub fn bound_mut(&mut self) -> &mut BoundQuery {
        &mut self.bound
    }
    pub fn set_stage(&mut self, stage: QueryRunStage) {
        self.stage = stage;
    }
    pub fn set_execution(&mut self, execution: QueryExecution) {
        self.bound = execution.bound.clone();
        self.execution = Some(execution);
    }
    pub fn execution(&self) -> Option<&QueryExecution> {
        self.execution.as_ref()
    }
    pub fn execution_mut(&mut self) -> Option<&mut QueryExecution> {
        self.execution.as_mut()
    }
    pub fn take_execution(&mut self) -> Result<QueryExecution> {
        self.execution.take().ok_or_else(|| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::FailedPrecondition,
                "Query result is unavailable",
            )
        })
    }
}

fn unavailable_run(reason: &'static str) -> Error {
    Error::new(
        ErrorOwner::Runtime,
        ErrorCode::StaleContext,
        "Query continuation is unavailable",
    )
    .with_context("reason", reason)
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

    pub fn expire_query_leases(&self) -> Result<()> {
        let mut runs = self.query_runs.lock().map_err(|_| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InternalFailure,
                "Query continuation state is unavailable",
            )
        })?;
        runs.retain(|_, run| run.lease.live());
        Ok(())
    }

    pub fn retain_query_run(&self, mut run: QueryRun) -> Result<QueryRunHandle> {
        run.lease.require_live()?;
        let mut runs = self.query_runs.lock().map_err(|_| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InternalFailure,
                "Query continuation state is unavailable",
            )
        })?;
        runs.retain(|_, value| value.lease.live());
        if runs.contains_key(&run.id) {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::Conflict,
                "Query continuation identity is unavailable",
            ));
        }
        if let Some(execution) = run.execution() {
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
        }
        if runs.len() >= run.bound().config_snapshot.get(crate::QUERY_SLOTS)? {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::CapacityExhausted,
                "Query capacity is exhausted",
            )
            .with_context("reason", "query_capacity"));
        }
        run.revision = run.revision.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InternalFailure,
                "Query continuation revision overflow",
            )
        })?;
        let handle = run.handle();
        runs.insert(run.id, run);
        Ok(handle)
    }

    pub fn take_query_run(&self, subject: SubjectId, handle: QueryRunHandle) -> Result<QueryRun> {
        let mut runs = self.query_runs.lock().map_err(|_| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InternalFailure,
                "Query continuation state is unavailable",
            )
        })?;
        let Some(run) = runs.get(&handle.query_run_id) else {
            return Err(unavailable_run("query_run_unavailable"));
        };
        if run.subject != subject {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "QueryRun subject does not match",
            ));
        }
        if run.revision != handle.revision {
            return Err(unavailable_run("query_run_stale_revision"));
        }
        if !run.lease.live() {
            runs.remove(&handle.query_run_id);
            return Err(unavailable_run("query_run_unavailable"));
        }
        runs.remove(&handle.query_run_id)
            .ok_or_else(|| unavailable_run("query_run_unavailable"))
    }

    /// Read-only owner-material access while one Core embedding handoff is
    /// outstanding. The caller neither consumes nor advances this revision.
    pub fn query_run_bound(
        &self,
        subject: SubjectId,
        handle: QueryRunHandle,
        expected_stage: QueryRunStage,
    ) -> Result<BoundQuery> {
        let mut runs = self.query_runs.lock().map_err(|_| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InternalFailure,
                "Query continuation state is unavailable",
            )
        })?;
        let Some(run) = runs.get(&handle.query_run_id) else {
            return Err(unavailable_run("query_run_unavailable"));
        };
        if run.subject != subject {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "QueryRun subject does not match",
            ));
        }
        if run.revision != handle.revision {
            return Err(unavailable_run("query_run_stale_revision"));
        }
        if !run.lease.live() {
            runs.remove(&handle.query_run_id);
            return Err(unavailable_run("query_run_unavailable"));
        }
        if run.stage != expected_stage {
            return Err(unavailable_run("query_run_stale_revision"));
        }
        Ok(run.bound().clone())
    }

    pub fn cancel_query_run(&self, subject: SubjectId, handle: QueryRunHandle) -> Result<()> {
        self.take_query_run(subject, handle).map(drop)
    }

    pub async fn finish_query_run(
        &self,
        mut run: QueryRun,
        order: Vec<(CognitiveRef, f64)>,
        external_results: Vec<ExternalResourceResult>,
        contributors: CognitiveContributors<'_>,
    ) -> Result<CognitiveQueryResult> {
        if run.stage() != QueryRunStage::Finalize {
            return Err(Error::new(
                ErrorOwner::Runtime,
                ErrorCode::FailedPrecondition,
                "Query continuation stage is unavailable",
            ));
        }
        let execution = run.take_execution()?;
        let lease = run.lease();
        let subject = run.subject();
        lease.require_live()?;
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
        lease.require_live()?;
        self.finalize_resources(subject, &mut result, external_results)
            .await?;
        self.record_finalized_query_feedback(&execution.bound, &result, Some(lease))
            .await?;
        Ok(result)
    }

    /// Complete a direct one-shot query execution at the same response fence as
    /// a QueryRun. Candidate pools must instead be retained and finalized by a
    /// QueryRun so only the final membership is recorded.
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
            self.record_finalized_query_feedback(&bound, &result, None)
                .await?;
            Ok(QueryExecution {
                read_lease,
                result,
                bound,
                leaves,
            })
        })
    }

    /// Complete a run that needed no host model or resource continuation.
    /// Query candidates were already validated during the frozen execution;
    /// only the purge fence and feedback remain at the response boundary.
    pub async fn complete_query_run(&self, mut run: QueryRun) -> Result<CognitiveQueryResult> {
        let execution = run.take_execution()?;
        let result = finalize_query_response(&execution.bound, execution.result);
        self.record_finalized_query_feedback(&execution.bound, &result, Some(run.lease()))
            .await?;
        Ok(result)
    }

    async fn record_finalized_query_feedback(
        &self,
        bound: &BoundQuery,
        result: &CognitiveQueryResult,
        lease: Option<QueryLease>,
    ) -> Result<()> {
        if let Some(lease) = lease {
            lease.require_live()?;
        }
        self.ensure_read_context_current(&bound.read_context)
            .await?;
        self.record_query_feedback(bound, result).await?;
        if let Some(lease) = lease {
            lease.require_live()?;
        }
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
