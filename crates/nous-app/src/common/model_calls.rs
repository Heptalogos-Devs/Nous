//! Paid invocation lifecycle shared by every model consumer.
use super::{
    clock::Clock,
    models::{
        AttemptContext, CallMetadata, ModelExecution, ModelFailure, ModelRuntime, PreparedCall,
        RetryClass,
    },
    tasks::Tasks,
};
use nous_domain::*;
use nous_storage::Database;
use serde::Serialize;
use serde_json::{Value, json};
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ModelCalls {
    database: Database,
    tasks: Tasks,
    clock: Clock,
}
impl ModelCalls {
    pub fn new(database: Database, tasks: Tasks, clock: Clock) -> Self {
        Self {
            database,
            tasks,
            clock,
        }
    }
    pub async fn recover(&self) -> Result<()> {
        self.database.recover_interrupted_calls().await?;
        Ok(())
    }
    pub async fn read(&self, ctx: &CallContext, id: &ModelCallId) -> Result<ModelCall> {
        let call = self.database.model_call(id).await?;
        ctx.require_subject(&call.namespace_id, &call.subject_id)?;
        Ok(call)
    }
    pub async fn ready(
        &self,
        job: &JobRecord,
        step: &str,
        runtime: &ModelRuntime,
        role: &str,
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
        runtime
            .ready_plan(
                &ConfigScope::Subject {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                },
                role,
                AttemptContext {
                    index,
                    previous_request: previous.map(|call| call.request),
                },
            )
            .await
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
                    let mut tx = self.tasks.operation("model.prepare", job_id).await?;
                    let mut job = tx.job_record(job_id).await?;
                    if job.state == JobState::Cancelled {
                        tx.rollback().await?;
                        return Ok(None);
                    }
                    let wait = (failure.class() == RetryClass::WaitingConfiguration).then(|| {
                        WaitCondition {
                            kind: "configuration".into(),
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
            if tx
                .daily_model_calls(&reservation.resource_id, &self.clock.budget_day())
                .await?
                >= i64::from(reservation.max_calls_per_day)
            {
                let tomorrow = self
                    .clock
                    .utc()
                    .date_naive()
                    .succ_opt()
                    .expect("next UTC date")
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight")
                    .and_utc()
                    .timestamp_millis();
                let wait = WaitCondition {
                    kind: "budget".into(),
                    details: json!({"resource_id":reservation.resource_id,"not_before_ms":tomorrow}),
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
            let attempts = tx.model_step_calls(job_id, step).await?.len() as i64;
            let mut call = ModelCall {
                id: reservation.call_id,
                job_id: job_id.into(),
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
                resource_id: reservation.resource_id,
                request: reservation.request,
                response: None,
                state: ModelCallState::Started,
                error: None,
                cost_microunits: None,
                created_at: self.clock.recorded(),
                completed_at: None,
            };
            tx.insert_model_call(&call).await?;
            tx.commit(json!({"call_id":call.id})).await?;

            let execution =
                tokio::select! {result=execute(prepared)=>Some(result),_=cancel.cancelled()=>None};
            let interrupted = execution.is_none();
            let mut retry_delay = None;
            match execution {
                Some(execution) => {
                    call.completed_at = Some(execution.record.completed_at.clone());
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
                        call.error = Some(failure.to_domain_error());
                    }
                }
                None => {
                    call.completed_at = Some(self.clock.recorded());
                    call.state = ModelCallState::Uncertain;
                    call.error = Some(Error::new(
                        ErrorCode::DependencyUnavailable,
                        "model request was interrupted after dispatch",
                    ));
                    call.response = Some(
                        json!({"record":{"started_at":call.created_at,"completed_at":call.completed_at,"status":"uncertain","usage":null,"cost":null},"result":null}),
                    );
                    retry_delay = Some(0);
                }
            }
            let mut tx = self.tasks.operation("model.complete", job_id).await?;
            let mut job = tx.job_record(job_id).await?;
            if job.state == JobState::Cancelled {
                let mut retired = tx.model_call(&call.id).await?;
                let source_retired = retired.request.get("redacted") == Some(&Value::Bool(true));
                if source_retired {
                    keep_accounting(&mut retired, &call);
                } else {
                    retired = call.clone();
                }
                tx.update_model_call(&retired).await?;
                tx.commit(json!({"call_id":call.id,"source_retired":source_retired}))
                    .await?;
                return Ok(None);
            }
            tx.update_model_call(&call).await?;
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

fn keep_accounting(current: &mut ModelCall, completed: &ModelCall) {
    let record = &completed.response.as_ref().unwrap_or(&Value::Null)["record"];
    let accounting: serde_json::Map<String, Value> = [
        "provider_model",
        "usage",
        "cost",
        "started_at",
        "completed_at",
        "status",
        "failure",
    ]
    .into_iter()
    .filter_map(|key| record.get(key).map(|value| (key.into(), value.clone())))
    .collect();
    current.state = completed.state.clone();
    current.error = completed.error.clone();
    current.completed_at = completed.completed_at.clone();
    current.cost_microunits = completed.cost_microunits;
    current.response = Some(json!({"redacted":true,"reason":"source_removed","record":accounting}));
}
