//! Paid invocation lifecycle shared by every model consumer.
mod admission;
use super::{
    clock::Clock,
    models::{
        AttemptContext, CallMetadata, ModelBudgets, ModelExecution, ModelFailure, ModelRuntime,
        PreparedCall, RetryClass,
    },
    tasks::Tasks,
};
pub use admission::{AdmissionDecision, AdmissionFuture, CallAdmission};
use nous_domain::*;
use nous_storage::Database;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

#[derive(Clone)]
pub struct ModelCalls {
    database: Database,
    tasks: Tasks,
    clock: Clock,
    budgets: ModelBudgets,
}
impl ModelCalls {
    pub fn new(database: Database, tasks: Tasks, clock: Clock, budgets: ModelBudgets) -> Self {
        Self {
            database,
            tasks,
            clock,
            budgets,
        }
    }
    pub async fn recover(&self) -> Result<()> {
        self.database.recover_interrupted_calls().await?;
        Ok(())
    }
    pub async fn read(&self, ctx: &CallContext, id: &ModelCallId) -> Result<ModelCall> {
        let mut view = self.database.read_view().await?;
        let call=self.read_in(&mut view,ctx,id).await?;
        view.finish().await?;
        Ok(call)
    }
    pub async fn read_in(&self,view:&mut nous_storage::ReadView,ctx:&CallContext,id:&ModelCallId)->Result<ModelCall> {
        let call = view.model_call(id).await?;
        ctx.require_subject(&call.namespace_id, &call.subject_id)?;
        if !ctx.source_read.is_empty() || matches!(&call.origin, ModelCallOrigin::Imported { .. }) {
            let metadata: super::models::InvocationMetadata<Value> =
                serde_json::from_value(call.request.clone()).map_err(|_| {
                    Error::new(
                        ErrorCode::Forbidden,
                        "model call has no available source requirements",
                    )
                })?;
            let sources = metadata.preview.sources.as_ref().ok_or_else(|| {
                Error::new(
                    ErrorCode::Forbidden,
                    "model call contains input without an attributable source catalog",
                )
            })?;
            let mapped = view.imported_model_sources(&call, sources).await?;
            view.require_source_requirements(ctx, &call.namespace_id, &call.subject_id, &mapped)
                .await?;
        }
        Ok(call)
    }
    pub async fn ready(
        &self,
        job: &JobRecord,
        step: &str,
        runtime: &ModelRuntime,
        role: &str,
        input: Option<Value>,
    ) -> bool {
        let Ok(previous) = self.database.latest_model_call(&job.id, step).await else {
            return false;
        };
        if previous
            .as_ref()
            .is_some_and(|call| call.state == ModelCallState::Succeeded)
        {
            return true;
        }
        let Ok((namespace, subject)) = job.subject_scope() else {
            return false;
        };
        let Ok(index) = previous
            .as_ref()
            .map_or(Ok(0), |call| u32::try_from(call.attempt))
        else {
            return false;
        };
        let Ok(mut resources) = runtime
            .ready_resources(
                &ConfigScope::Subject {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                },
                role,
                AttemptContext {
                    index,
                    previous_request: previous.map(|call| call.request),
                },
                input.as_ref(),
            )
            .await
        else {
            return false;
        };
        if let Some(resource) = job
            .wait
            .as_ref()
            .filter(|wait| wait.kind == "budget")
            .and_then(|wait| wait.details.get("resource"))
            .and_then(|value| {
                serde_json::from_value::<super::models::ResourceMetadata>(value.clone()).ok()
            })
            && resources
                .iter()
                .any(|ready| ready.id == resource.id && ready.provider == resource.provider)
        {
            resources = vec![resource];
        }
        let day = self.clock.utc().date_naive().to_string();
        for resource in resources {
            match self
                .budgets
                .has_capacity(&self.database, job, role, &resource, &day)
                .await
            {
                Ok(true) => return true,
                Ok(false) => {}
                Err(error) => {
                    tracing::debug!(trace_id=%job.trace_id,job_id=%job.id,%error,"model budget readiness unavailable")
                }
            }
        }
        false
    }
    /// Preparation is lazy: a completed step is reused before reading current
    /// credentials or making another provider request.
    pub async fn invoke<M, T, P, PF, E, EF>(
        &self,
        job_id: &str,
        step: &str,
        cancel: &CancellationToken,
        mut prepare: P,
        execute: E,
        admission: Option<&dyn CallAdmission<M>>,
    ) -> Result<Option<ModelCall>>
    where
        M: CallMetadata + Serialize,
        T: Serialize,
        P: FnMut(AttemptContext) -> PF,
        PF: Future<Output = std::result::Result<PreparedCall<M>, ModelFailure>>,
        E: Fn(PreparedCall<M>) -> EF,
        EF: Future<Output = ModelExecution<M, T>>,
    {
        validate_name(step, "model step key")?;
        loop {
            if cancel.is_cancelled() {
                return self.defer_interrupted(job_id).await;
            }
            let job = self.database.job_record(job_id).await?;
            if job.state == JobState::Cancelled {
                return Ok(None);
            }
            let previous = self.database.latest_model_call(job_id, step).await?;
            if let Some(call) = &previous {
                match call.state {
                    ModelCallState::Succeeded => return Ok(Some(call.clone())),
                    ModelCallState::Started => {
                        return Err(Error::new(
                            ErrorCode::Conflict,
                            "model step is already running",
                        ));
                    }
                    _ => {}
                }
            }
            let index = previous.as_ref().map_or(Ok(0), |call| {
                u32::try_from(call.attempt)
                    .map_err(|_| Error::internal("invalid model attempt number"))
            })?;
            let context = AttemptContext {
                index,
                previous_request: previous.as_ref().map(|call| call.request.clone()),
            };
            let prepared = match prepare(context).await {
                Ok(prepared) => prepared,
                Err(failure) => {
                    if cancel.is_cancelled() {
                        return self.defer_interrupted(job_id).await;
                    }
                    let mut tx = self.tasks.operation("model.prepare", job_id).await?;
                    let mut job = tx.job_record(job_id).await?;
                    if job.state == JobState::Cancelled {
                        tx.rollback().await?;
                        return Ok(None);
                    }
                    let wait = (failure.class() == RetryClass::WaitingConfiguration).then(|| {
                        WaitCondition {
                            kind: "configuration".into(),
                            requires_attention: true,
                            resume: ResumeCondition::Ready,
                            details: json!({"task_kind":job.kind}),
                        }
                    });
                    let state = if wait.is_some() {
                        JobState::Waiting
                    } else {
                        JobState::Failed
                    };
                    self.tasks
                        .change_in(
                            &mut tx,
                            &mut job,
                            state,
                            wait,
                            Some(failure.to_domain_error()),
                        )
                        .await?;
                    tx.commit(json!({"state":job.state})).await?;
                    return Ok(None);
                }
            };
            let reservation = prepared.reservation();
            let policy = reservation.attempt_policy;
            if index >= policy.max_attempts
                || (previous
                    .as_ref()
                    .is_some_and(|call| call.state == ModelCallState::Uncertain)
                    && !policy.retry_uncertain)
            {
                let error = previous
                    .as_ref()
                    .and_then(|call| call.error.clone())
                    .unwrap_or_else(|| {
                        Error::new(
                            ErrorCode::ResourceExhausted,
                            "model invocation attempt policy is exhausted",
                        )
                    });
                let mut tx = self.tasks.operation("model.policy", job_id).await?;
                let mut current = tx.job_record(job_id).await?;
                if current.state != JobState::Cancelled {
                    self.tasks
                        .change_in(&mut tx, &mut current, JobState::Failed, None, Some(error))
                        .await?;
                }
                tx.commit(json!({"state":current.state})).await?;
                return Ok(None);
            }
            let mut tx = self.tasks.operation("model.reserve", job_id).await?;
            let mut job = tx.job_record(job_id).await?;
            if job.state == JobState::Cancelled {
                tx.rollback().await?;
                return Ok(None);
            }
            if let Some(previous) = tx.latest_model_call(job_id, step).await? {
                if previous.state == ModelCallState::Succeeded {
                    tx.rollback().await?;
                    return Ok(Some(previous));
                }
                if previous.state == ModelCallState::Started {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "model step is already running",
                    ));
                }
            }
            if let Some(admission) = admission
                && matches!(
                    admission.validate(&mut tx, &prepared.metadata).await?,
                    AdmissionDecision::Refresh
                )
            {
                tx.rollback().await?;
                continue;
            }
            if !job.source_read.is_empty() {
                let sources = prepared.metadata.source_requirements().ok_or_else(|| {
                    Error::new(
                        ErrorCode::Forbidden,
                        "scoped model execution requires an attributable source catalog",
                    )
                })?;
                let context = job.call_context("model-source")?;
                let (namespace, subject) = job.subject_scope()?;
                tx.require_source_requirements(&context, namespace, subject, sources)
                    .await?;
            }
            let budget_plan = match self
                .budgets
                .plan_in(&mut tx, &job, &reservation.role, &reservation.resource)
                .await
            {
                Ok(plan) => plan,
                Err(error) if error.code == ErrorCode::CapabilityUnavailable => {
                    let wait = WaitCondition {
                        kind: "configuration".into(),
                        requires_attention: true,
                        resume: ResumeCondition::Ready,
                        details: json!({"role":reservation.role,"resource_id":reservation.resource.id}),
                    };
                    self.tasks
                        .change_in(
                            &mut tx,
                            &mut job,
                            JobState::Waiting,
                            Some(wait),
                            Some(error),
                        )
                        .await?;
                    tx.commit(json!({"state":job.state})).await?;
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            let attempts = tx.model_step_calls(job_id, step).await?.len() as i64;
            let reserved_at = tx.time().utc();
            let mut call = ModelCall {
                origin: nous_domain::ModelCallOrigin::Local,
                id: reservation.call_id,
                job_id: job_id.into(),
                trace_id: job.trace_id.clone(),
                step_key: step.into(),
                namespace_id: job
                    .namespace_id
                    .clone()
                    .ok_or_else(|| Error::invalid("model invocation needs namespace scope"))?,
                subject_id: job
                    .subject_id
                    .clone()
                    .ok_or_else(|| Error::invalid("model invocation needs subject scope"))?,
                attempt: attempts + 1,
                resource_id: reservation.resource.id.clone(),
                request: reservation.request,
                response: None,
                state: ModelCallState::Started,
                error: None,
                usage: None,
                cost: None,
                created_at: reserved_at.to_rfc3339(),
                completed_at: None,
            };
            match tx.reserve_model_call(&call, &budget_plan).await? {
                nous_storage::ModelBudgetAdmission::Reserved => {}
                nous_storage::ModelBudgetAdmission::AlreadyReserved => {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "model call is already reserved",
                    ));
                }
                nous_storage::ModelBudgetAdmission::Exhausted(buckets) => {
                    let tomorrow = reserved_at
                        .date_naive()
                        .succ_opt()
                        .expect("next UTC date")
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight")
                        .and_utc()
                        .timestamp_millis();
                    let wait = WaitCondition {
                        kind: "budget".into(),
                        requires_attention: true,
                        resume: ResumeCondition::ReadyOrNotBefore { unix_ms: tomorrow },
                        details: json!({"buckets":buckets,"resource":reservation.resource,"role":reservation.role}),
                    };
                    self.tasks
                        .change_in(
                            &mut tx,
                            &mut job,
                            JobState::Waiting,
                            Some(wait),
                            Some(Error::new(
                                ErrorCode::ResourceExhausted,
                                "daily model call budget is exhausted",
                            )),
                        )
                        .await?;
                    tx.commit(json!({"state":job.state})).await?;
                    return Ok(None);
                }
            }
            tx.commit(json!({"call_id":call.id})).await?;

            let started = Instant::now();
            let span = tracing::info_span!("model.attempt", trace_id=%call.trace_id, job_id=%call.job_id, call_id=%call.id, step=%call.step_key, role=%reservation.role, resource=%call.resource_id, attempt=call.attempt);
            let execution = crate::observability::in_trace(call.trace_id.clone(), async {
                tracing::info!(event="model.dispatched", "model request dispatched");
                tokio::select! {result=execute(prepared)=>Some(result),_=cancel.cancelled()=>None}
            }.instrument(span.clone())).await;
            let interrupted = execution.is_none();
            let mut retry_delay = None;
            match execution {
                Some(execution) => {
                    call.completed_at = Some(execution.record.completed_at.clone());
                    call.usage = execution.record.usage.clone();
                    call.cost = execution.record.cost.clone();
                    let (output, failure) = match execution.result {
                        Ok(output) => (Some(serde_json::to_value(output)?), None),
                        Err(failure) => (None, Some(failure)),
                    };
                    call.response = Some(json!({"record":execution.record,"result":output}));
                    call.state = match &failure {
                        Some(failure) if failure.class() == RetryClass::Uncertain => {
                            ModelCallState::Uncertain
                        }
                        Some(_) => ModelCallState::Failed,
                        None => ModelCallState::Succeeded,
                    };
                    if let Some(failure) = failure {
                        let allowed = match failure.class() {
                            RetryClass::Permanent => policy.has_fallback,
                            RetryClass::Uncertain => policy.retry_uncertain,
                            RetryClass::Retryable => true,
                            RetryClass::WaitingConfiguration => false,
                        };
                        if allowed && call.attempt < i64::from(policy.max_attempts) {
                            retry_delay =
                                Some(failure.info.retry_after_seconds.unwrap_or(2).min(30));
                        }
                        call.error =
                            Some(failure.to_domain_error().with_trace(call.trace_id.clone()));
                    }
                }
                None => {
                    call.completed_at = Some(self.clock.recorded());
                    call.state = ModelCallState::Uncertain;
                    call.error = Some(
                        Error::new(
                            ErrorCode::DependencyUnavailable,
                            "model request was interrupted after dispatch",
                        )
                        .with_trace(call.trace_id.clone()),
                    );
                    call.response = Some(
                        json!({"record":{"started_at":call.created_at,"completed_at":call.completed_at,"status":"uncertain","usage":null,"cost":null},"result":null}),
                    );
                    retry_delay = Some(0);
                }
            }
            let status = match call.state {
                ModelCallState::Started => "started",
                ModelCallState::Succeeded => "succeeded",
                ModelCallState::Failed => "failed",
                ModelCallState::Uncertain => "uncertain",
            };
            let elapsed = started.elapsed();
            metrics::counter!("nous_model_attempts_total", "role"=>reservation.role.clone(), "status"=>status).increment(1);
            record_usage_metrics(&reservation.role, call.usage.as_ref());
            metrics::histogram!("nous_model_attempt_duration_seconds", "role"=>reservation.role)
                .record(elapsed.as_secs_f64());
            tracing::info!(parent:&span, event="model.completed", status, elapsed_ms=elapsed.as_millis() as u64, error=?call.error, "model request completed");
            let mut tx = self.tasks.operation("model.complete", job_id).await?;
            let mut job = tx.job_record(job_id).await?;
            call = tx.merge_model_call_completion(&call).await?;
            if job.state == JobState::Cancelled {
                tx.commit(json!({"call_id":call.id,"state":job.state}))
                    .await?;
                return Ok(None);
            }
            if call.state != ModelCallState::Succeeded {
                self.tasks
                    .change_in(
                        &mut tx,
                        &mut job,
                        if interrupted {
                            JobState::Queued
                        } else if retry_delay.is_some() {
                            JobState::Running
                        } else {
                            JobState::Failed
                        },
                        None,
                        call.error.clone(),
                    )
                    .await?;
            }
            tx.commit(json!({"call_id":call.id,"state":call.state}))
                .await?;
            if interrupted {
                return Err(call.error.clone().expect("interrupted call has an error"));
            }
            if let Some(delay) = retry_delay {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(delay)) => {},
                    _ = cancel.cancelled() => return self.defer_interrupted(job_id).await,
                }
                continue;
            }
            return Ok((call.state == ModelCallState::Succeeded).then_some(call));
        }
    }

    async fn defer_interrupted(&self, job_id: &str) -> Result<Option<ModelCall>> {
        let mut tx = self.tasks.operation("model.interrupted", job_id).await?;
        let mut job = tx.job_record(job_id).await?;
        if matches!(
            job.state,
            JobState::Cancelled | JobState::Succeeded | JobState::Failed
        ) {
            tx.rollback().await?;
            return Ok(None);
        }
        let error = Error::new(
            ErrorCode::DependencyUnavailable,
            "model invocation was interrupted",
        );
        self.tasks
            .change_in(
                &mut tx,
                &mut job,
                JobState::Queued,
                None,
                Some(error.clone()),
            )
            .await?;
        tx.commit(json!({"state":job.state})).await?;
        Err(error)
    }
}

fn record_usage_metrics(role: &str, usage: Option<&ModelUsage>) {
    let Some(usage) = usage else {
        metrics::counter!("nous_model_usage_missing_total","role"=>role.to_owned()).increment(1);
        return;
    };
    if usage.input_tokens.is_none()
        || (usage.kind == UsageKind::Chat && usage.output_tokens.is_none())
    {
        metrics::counter!("nous_model_usage_incomplete_total","role"=>role.to_owned()).increment(1);
    }
    if let Some(count) = usage.input_tokens {
        metrics::counter!("nous_model_input_tokens_total","role"=>role.to_owned()).increment(count);
    }
    if let Some(count) = usage.output_tokens {
        metrics::counter!("nous_model_output_tokens_total","role"=>role.to_owned())
            .increment(count);
    }
    if let Some(count) = usage.known_total_tokens() {
        metrics::counter!("nous_model_tokens_total","role"=>role.to_owned()).increment(count);
    }
}
