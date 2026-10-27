mod capture;
pub(crate) mod config;
pub(crate) mod model;
mod recheck;
mod worker;
use crate::App;
use nous_domain::*;
use nous_storage::{BeginOperation, operation_key};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

impl App {
    pub(super) async fn rerank_hits(
        &self,
        ctx: &CallContext,
        plan: &nous_domain::nousql::QueryPlan,
        policy: &super::config::RetrievalPolicy,
        hits: &mut [QueryHit],
    ) -> Result<Option<QueryRerankInfo>> {
        if plan.rerank == QueryRerankMode::Off {
            return Ok(None);
        }
        let mut info = QueryRerankInfo {
            mode: plan.rerank,
            status: QueryRerankStatus::Skipped,
            job_id: None,
            model_call_id: None,
            submitted_documents: 0,
            scored_documents: 0,
            retained_base_order: hits.len(),
            reason: None,
            error: None,
        };
        if hits.len() < 2
            || plan.sort.is_some()
            || plan
                .leaves
                .iter()
                .all(|leaf| leaf.direct_only || leaf.request.input.text.trim().is_empty())
        {
            info.reason = Some("direct_or_explicit_order".into());
            return Ok(Some(info));
        }
        let run = self.capture_rerank(ctx, plan, policy, hits).await?;
        if run.candidates.len() < 2 || run.query.trim().is_empty() {
            if plan.rerank == QueryRerankMode::Required {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "rerank requires at least two eligible text projections within the configured input budget",
                ));
            }
            info.reason = Some("insufficient_text_candidates".into());
            return Ok(Some(info));
        }
        let subject = &plan.leaves[0].request.subject_id;
        let ns = self.database.subject_namespace(subject).await?;
        let scope = format!("namespace:{ns}/subject:{subject}");
        let key = operation_key(
            ctx,
            "query.rerank",
            &scope,
            &ulid::Ulid::generate().to_string(),
            &json!(run),
        )?;
        let BeginOperation::New(mut tx) = self.database.begin_operation(key).await? else {
            unreachable!("fresh rerank acceptance")
        };
        for candidate in &run.candidates {
            tx.require_query_target(&ns, subject, &candidate.reference, candidate.current_head)
                .await?;
        }
        let mut job = self
            .tasks
            .new_job(&tx, "query.rerank", Some(ns), Some(subject.clone()));
        job.checkpoint = Some(json!(run));
        self.tasks.submit_in(&mut tx, &job).await?;
        tx.commit(json!({"job_id":job.id})).await?;
        info.job_id = Some(job.id.clone());
        info.submitted_documents = run.candidates.len();
        let cancel = CancellationToken::new();
        let outcome = self
            .tasks
            .wait_all(
                ctx,
                std::slice::from_ref(&job.id),
                Duration::from_millis(policy.rerank.wait_ms),
                &cancel,
            )
            .await;
        let current = match outcome {
            Ok(mut jobs) => jobs.remove(0),
            Err(error) => return self.rerank_fallback(ctx, info, error).await,
        };
        if current.state != JobState::Succeeded {
            let error = current.error.unwrap_or_else(|| {
                Error::new(
                    ErrorCode::DependencyUnavailable,
                    "rerank is waiting for model resources or budget",
                )
                .with_context("wait", json!(current.wait))
            });
            return self.rerank_fallback(ctx, info, error).await;
        }
        let result = current
            .result
            .ok_or_else(|| Error::internal("successful rerank has no result"))?;
        let call_id: ModelCallId = serde_json::from_value(result["call_id"].clone())?;
        // Recheck the same canonical source roster before exposing the call or its output.
        let call = match self.calls.read(ctx, &call_id).await {
            Ok(call) => call,
            Err(error) => return self.rerank_fallback(ctx, info, error).await,
        };
        if call
            .request
            .get("redacted")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            return self
                .rerank_fallback(
                    ctx,
                    info,
                    Error::new(ErrorCode::NotFound, "rerank source was purged"),
                )
                .await;
        }
        let scores: Vec<nous_foundation::models::RerankScore> =
            serde_json::from_value(result["scores"].clone())?;
        let mut order = BTreeMap::new();
        for score in &scores {
            let candidate = run
                .candidates
                .get(score.index)
                .ok_or_else(|| Error::internal("rerank index exceeds accepted catalog"))?;
            order.insert(candidate.reference.clone(), score.relevance_score);
        }
        for hit in hits.iter_mut() {
            hit.rerank_score = order.get(&hit.reference).copied();
        }
        // Stable sorting retains the base ranking for unscored documents and ties.
        hits.sort_by(
            |left, right| match (left.rerank_score, right.rerank_score) {
                (Some(left), Some(right)) => right.total_cmp(&left),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        );
        info.status = QueryRerankStatus::Completed;
        info.model_call_id = Some(call_id);
        info.scored_documents = scores.len();
        info.retained_base_order = hits.len().saturating_sub(scores.len());
        Ok(Some(info))
    }
    async fn rerank_fallback(
        &self,
        ctx: &CallContext,
        mut info: QueryRerankInfo,
        error: Error,
    ) -> Result<Option<QueryRerankInfo>> {
        if info.mode == QueryRerankMode::Required {
            return Err(error.with_context("rerank", json!(info)));
        }
        if let Some(id) = &info.job_id {
            self.tasks
                .cancel(ctx, &ulid::Ulid::generate().to_string(), id)
                .await?;
        }
        info.status = QueryRerankStatus::Fallback;
        info.error = Some(error);
        Ok(Some(info))
    }
}
