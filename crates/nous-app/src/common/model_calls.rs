//! Paid invocation lifecycle shared by every model consumer.
use super::{
    clock::Clock,
    models::{CallMetadata, ModelExecution, ModelFailure, PreparedCall, RetryClass},
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
    /// Preparation is lazy: a completed step is reused before reading current
    /// credentials or making another provider request.
    pub async fn invoke<M, T, P, PF, E, EF>(
        &self,
        job_id: &str,
        step: &str,
        cancel: &CancellationToken,
        prepare: P,
        execute: E,
    ) -> Result<Option<ModelCall>>
    where
        M: CallMetadata + Serialize,
        T: Serialize,
        P: FnOnce() -> PF,
        PF: Future<Output = std::result::Result<PreparedCall<M>, ModelFailure>>,
        E: FnOnce(PreparedCall<M>) -> EF,
        EF: Future<Output = ModelExecution<M, T>>,
    {
        validate_name(step, "model step key")?;
        let job = self.database.job_record(job_id).await?;
        if job.state == JobState::Cancelled {
            return Ok(None);
        }
        if let Some(call) = self.database.latest_model_call(job_id, step).await? {
            match call.state {
                ModelCallState::Succeeded => return Ok(Some(call)),
                ModelCallState::Started => {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "model step is already running",
                    ));
                }
                _ => {}
            }
        }
        let prepared = match prepare().await {
            Ok(prepared) => prepared,
            Err(failure) => {
                let mut tx = self.tasks.operation("model.prepare", job_id).await?;
                let mut job = tx.job_record(job_id).await?;
                if job.state == JobState::Cancelled {
                    tx.rollback().await?;
                    return Ok(None);
                }
                let wait =
                    (failure.class() == RetryClass::WaitingConfiguration).then(|| WaitCondition {
                        kind: "configuration".into(),
                        details: json!({"task_kind":job.kind}),
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
                    if failure.class() != RetryClass::Permanent && call.attempt < 5 {
                        retry_delay = Some(failure.info.retry_after_seconds.unwrap_or(2).min(30));
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
                    if retry_delay.is_some() {
                        JobState::Queued
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
        if let Some(delay) = retry_delay {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            return Err(call.error.clone().unwrap_or_else(|| {
                Error::new(ErrorCode::DependencyUnavailable, "model request can retry")
            }));
        }
        Ok((call.state == ModelCallState::Succeeded).then_some(call))
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
