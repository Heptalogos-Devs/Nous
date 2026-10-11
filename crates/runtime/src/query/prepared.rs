// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{BoundQuery, CognitiveContributors, QueryExecution};
use crate::CognitiveRuntimeService;
use nous_core::*;
use std::collections::{HashMap, HashSet, hash_map::Entry};
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
            return Err(Error::Invalid(
                "query opportunity must be 1ms..3600 seconds".into(),
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
            Err(Error::Unavailable(
                "query execution opportunity expired".into(),
            ))
        }
    }
    fn live(self) -> bool {
        Instant::now() < self.deadline
    }
}
#[derive(Debug, Clone)]
pub struct QueryReservation {
    pub bound: BoundQuery,
    lease: QueryLease,
}
impl QueryReservation {
    pub fn new(bound: BoundQuery, lease: QueryLease) -> Result<Self> {
        lease.require_live()?;
        Ok(Self { bound, lease })
    }
    pub fn into_parts(self) -> (BoundQuery, QueryLease) {
        (self.bound, self.lease)
    }
}

pub(crate) enum PendingQuery {
    Prepared(Box<QueryReservation>),
    Executed {
        execution: Box<QueryExecution>,
        lease: QueryLease,
    },
}
impl PendingQuery {
    fn bound(&self) -> &BoundQuery {
        match self {
            Self::Prepared(reservation) => &reservation.bound,
            Self::Executed { execution, .. } => &execution.bound,
        }
    }
    fn lease(&self) -> QueryLease {
        match self {
            Self::Prepared(reservation) => reservation.lease,
            Self::Executed { lease, .. } => *lease,
        }
    }
}

impl CognitiveRuntimeService {
    pub fn expire_query_leases(&self) -> Result<()> {
        let mut pending = self
            .pending_queries
            .lock()
            .map_err(|_| Error::Infrastructure("query lease lock unavailable".into()))?;
        pending.retain(|_, value| value.lease().live());
        Ok(())
    }

    pub fn retain_prepared_query(&self, reservation: QueryReservation) -> Result<Uuid> {
        reservation.lease.require_live()?;
        let bound = &reservation.bound;
        let mut pending = self
            .pending_queries
            .lock()
            .map_err(|_| Error::Infrastructure("query lease lock unavailable".into()))?;
        pending.retain(|_, value| value.lease().live());
        if pending.len() >= bound.config_snapshot.get(crate::QUERY_SLOTS)? {
            return Err(Error::Unavailable("prepared query slots are busy".into()));
        }
        let token = Uuid::new_v4();
        pending.insert(token, PendingQuery::Prepared(Box::new(reservation)));
        Ok(token)
    }
    pub fn prepared_query(&self, subject: SubjectId, token: Uuid) -> Result<BoundQuery> {
        let pending = self
            .pending_queries
            .lock()
            .map_err(|_| Error::Infrastructure("query lease lock unavailable".into()))?;
        let entry = pending
            .get(&token)
            .ok_or_else(|| Error::NotFound("prepared query expired or consumed".into()))?;
        if entry.bound().source_query.subject != subject {
            return Err(Error::Invalid(
                "prepared query belongs to another Subject".into(),
            ));
        }
        entry.lease().require_live()?;
        match entry {
            PendingQuery::Prepared(reservation) => Ok(reservation.bound.clone()),
            PendingQuery::Executed { .. } => Err(Error::Invalid(
                "execution ticket is not a preparation token".into(),
            )),
        }
    }
    pub fn take_prepared_query(&self, subject: SubjectId, token: Uuid) -> Result<QueryReservation> {
        let mut pending = self
            .pending_queries
            .lock()
            .map_err(|_| Error::Infrastructure("query lease lock unavailable".into()))?;
        let entry = pending
            .get(&token)
            .ok_or_else(|| Error::NotFound("prepared query expired or consumed".into()))?;
        if entry.bound().source_query.subject != subject {
            return Err(Error::Invalid(
                "prepared query belongs to another Subject".into(),
            ));
        }
        if !matches!(entry, PendingQuery::Prepared(_)) {
            return Err(Error::Invalid(
                "execution ticket is not a preparation token".into(),
            ));
        }
        let value = pending
            .remove(&token)
            .ok_or_else(|| Error::NotFound("prepared query disappeared".into()))?;
        value.lease().require_live()?;
        match value {
            PendingQuery::Prepared(reservation) => Ok(*reservation),
            PendingQuery::Executed { .. } => Err(Error::Invalid("not a preparation token".into())),
        }
    }

    pub fn retain_query(
        &self,
        execution: QueryExecution,
        lease: QueryLease,
    ) -> Result<(CognitiveQueryResult, Option<Uuid>)> {
        lease.require_live()?;
        if execution.result.results.len() > 64 {
            return Err(Error::Invalid("rerank pool exceeds 64 candidates".into()));
        }
        if execution
            .result
            .results
            .iter()
            .map(|hit| hit.representation.as_ref().map_or(0, String::len))
            .sum::<usize>()
            > 2 * 1024 * 1024
        {
            return Ok(retention_fallback(
                execution,
                "validated pool exceeds the text retention bound",
            ));
        }
        let mut pending = self
            .pending_queries
            .lock()
            .map_err(|_| Error::Infrastructure("query lease lock unavailable".into()))?;
        pending.retain(|_, value| value.lease().live());
        if pending.len() >= execution.bound.config_snapshot.get(crate::QUERY_SLOTS)? {
            return Ok(retention_fallback(
                execution,
                "bounded query lease slots are busy",
            ));
        }
        let ticket = Uuid::new_v4();
        let result = execution.result.clone();
        pending.insert(
            ticket,
            PendingQuery::Executed {
                execution: Box::new(execution),
                lease,
            },
        );
        Ok((result, Some(ticket)))
    }
    fn take_query(&self, subject: SubjectId, ticket: Uuid) -> Result<(QueryExecution, QueryLease)> {
        let mut pending = self
            .pending_queries
            .lock()
            .map_err(|_| Error::Infrastructure("query lease lock unavailable".into()))?;
        let Entry::Occupied(entry) = pending.entry(ticket) else {
            return Err(Error::NotFound("query lease expired or consumed".into()));
        };
        if entry.get().bound().source_query.subject != subject {
            return Err(Error::Invalid(
                "query lease belongs to a different Subject".into(),
            ));
        }
        if !matches!(entry.get(), PendingQuery::Executed { .. }) {
            return Err(Error::Invalid("query ticket has not executed".into()));
        }
        let value = entry.remove();
        value.lease().require_live()?;
        match value {
            PendingQuery::Executed { execution, lease } => Ok((*execution, lease)),
            PendingQuery::Prepared(_) => {
                Err(Error::Invalid("query ticket has not executed".into()))
            }
        }
    }
    pub fn release_query(&self, subject: SubjectId, ticket: Uuid) -> Result<()> {
        let mut pending = self
            .pending_queries
            .lock()
            .map_err(|_| Error::Infrastructure("query lease lock unavailable".into()))?;
        if pending
            .get(&ticket)
            .is_some_and(|value| value.bound().source_query.subject != subject)
        {
            return Err(Error::Invalid(
                "query lease belongs to a different Subject".into(),
            ));
        }
        pending.remove(&ticket);
        Ok(())
    }
    pub async fn finalize_query(
        &self,
        subject: SubjectId,
        ticket: Uuid,
        order: Vec<(CognitiveRef, f64)>,
        external_results: Vec<ExternalResourceResult>,
        contributors: CognitiveContributors<'_>,
    ) -> Result<CognitiveQueryResult> {
        let (execution, lease) = self.take_query(subject, ticket)?;
        if order.len() > 64 || order.iter().any(|(_, score)| !score.is_finite()) {
            return Err(Error::Invalid(
                "rerank mapping exceeds bounds or has a nonfinite score".into(),
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
            return Err(Error::Invalid(
                "rerank mapping contains duplicate or unknown candidates".into(),
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
        result
            .results
            .truncate(execution.bound.source_query.result_need.limit);
        for (rank, hit) in result.results.iter_mut().enumerate() {
            hit.match_evidence.final_rank = (rank + 1) as u32;
        }
        lease.require_live()?;
        self.finalize_resources(subject, &mut result, external_results)
            .await?;
        lease.require_live()?;
        self.record_query_feedback(&execution.bound, &result)
            .await?;
        Ok(result)
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
                Ok(_) | Err(Error::NotFound(_)) | Err(Error::Invalid(_)) => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        match self.store.validate_reference(subject, reference).await {
            Ok(()) => Ok(super::orchestrate::generic_constraints_match(
                &bound.source_query.expression.constraints,
            )),
            Err(Error::NotFound(_)) | Err(Error::Invalid(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

fn same_stamp(current: Option<&CognitiveHit>, old: &super::types::CandidateStamp) -> bool {
    current.is_some_and(|current| {
        current.authority_epoch == old.authority_epoch && current.revision == old.revision
    })
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

fn retention_fallback(
    mut execution: QueryExecution,
    reason: &str,
) -> (CognitiveQueryResult, Option<Uuid>) {
    execution
        .result
        .results
        .truncate(execution.bound.source_query.result_need.limit);
    execution.result.degradation.push(Degradation {
        code: "query_rerank_pool_unavailable".into(),
        detail: Some(reason.into()),
    });
    if execution.result.status == QueryStatus::Complete {
        execution.result.status = QueryStatus::Degraded;
    }
    (execution.result, None)
}
