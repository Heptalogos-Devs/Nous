//! Adapted from apalis-sqlite examples/basic.rs and apalis Worker::run_until.
//! Upstream MIT licenses and pinned sources are recorded in research/reuse.md.
use super::*;
use apalis::prelude::{BackendExt, Data, WorkerBuilder, WorkerBuilderExt};
use apalis_core::error::WorkerError;
use cognition::{index_job, utc_point};
use model::{FormationInput, RetryClass};
use nous_storage::{JobEnvelope, WriteTransaction};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl App {
    /// The runtime host calls this only after acquiring the instance lock.
    /// The mature queue owns polling, leases, ACK and at-least-once delivery.
    pub fn start_background(&self, cancellation: CancellationToken) -> tokio::task::JoinHandle<()> {
        let app = self.clone();
        tokio::spawn(async move {
            if let Err(error) = app.database.recover_interrupted_jobs().await {
                tracing::error!(%error,"cannot recover durable work");
                return;
            }
            if let Err(error) = app.database.resume_waiting_formations().await {
                tracing::error!(%error,"cannot resume waiting formations");
            }
            if let Err(error) = app.catch_up_index().await {
                tracing::warn!(%error,"index catch-up will retry");
            }
            let maintenance_app = app.clone();
            let maintenance_cancel = cancellation.clone();
            let maintenance = async move {
                let mut budget_day = Utc::now().date_naive();
                loop {
                    tokio::select! {
                        _=maintenance_cancel.cancelled()=>break,
                        _=tokio::time::sleep(Duration::from_secs(60))=>{
                            let today = Utc::now().date_naive();
                            if today != budget_day {
                                budget_day = today;
                                if let Err(error)=maintenance_app.database.resume_waiting_formations().await {tracing::warn!(%error,"daily budget work resume failed");}
                            } else if maintenance_app.model.inspect()["state"] == "ready"
                                && let Err(error)=maintenance_app.database.resume_waiting_configurations().await {
                                tracing::warn!(%error,"configuration work resume failed");
                            }
                            if let Err(error)=maintenance_app.catch_up_index().await {tracing::warn!(%error,"index catch-up failed");}
                        }
                    }
                }
            };
            let backend = app
                .database
                .queue_backend()
                .poll_with_interval(Duration::from_millis(250));
            let worker = WorkerBuilder::new(format!("nous-worker-{}", ulid::Ulid::generate()))
                .backend(backend)
                .data(app.clone())
                .concurrency(1)
                .build(dispatch_job);
            let signal = async move {
                cancellation.cancelled().await;
                Ok::<(), WorkerError>(())
            };
            let running = async move {
                if let Err(error) = worker.run_until(signal).await {
                    tracing::error!(%error,"durable worker stopped");
                }
            };
            tokio::join!(running, maintenance);
        })
    }

    async fn process_formation(&self, job_id: &str) -> Result<()> {
        let mut run = self.database.formation(job_id).await?;
        if matches!(
            run.state,
            FormationState::Succeeded | FormationState::Cancelled | FormationState::Failed
        ) {
            return Ok(());
        }
        if let Some(id) = &run.model_call_id {
            let previous = self.database.model_call(id).await?;
            if previous.state == ModelCallState::Succeeded {
                return self.adopt_formation(&run, &previous).await;
            }
            if previous.state == ModelCallState::Started {
                self.mark_interrupted_call(previous).await?;
            }
        }
        let subject = self
            .database
            .subject(&run.namespace_id, &run.subject_id)
            .await?;
        let observation = self
            .database
            .observation(&run.namespace_id, &run.observation_id)
            .await?;
        let source = self
            .get_material(&CallContext::admin(), &run.material_id)
            .await?;
        if subject.revision != run.subject_revision
            || source.material.content_hash != run.material_hash
            || observation.subject_id != run.subject_id
        {
            self.set_formation_state(
                &run,
                FormationState::Failed,
                Error::new(ErrorCode::Conflict, "formation input version is stale"),
            )
            .await?;
            return Ok(());
        }
        let prepared = match self.model.prepare(FormationInput {
            namespace_id: run.namespace_id.clone(),
            subject_id: run.subject_id.clone(),
            material_id: run.material_id.clone(),
            material_commit_seq: source.material.commit_seq,
            observation_id: run.observation_id.clone(),
            observation_commit_seq: observation.commit_seq,
            source_event_kind: observation.kind,
            text: source.text,
        }) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let state = if failure.class() == RetryClass::WaitingConfiguration {
                    FormationState::WaitingForConfiguration
                } else {
                    FormationState::Failed
                };
                self.set_formation_state(&run, state, failure.to_domain_error())
                    .await?;
                return Ok(());
            }
        };
        let metadata = &prepared.metadata;
        let call_id: ModelCallId = metadata.call_id.parse()?;
        let previous_calls = self.database.model_calls(job_id).await?;
        let attempt = previous_calls.len() as i64 + 1;
        let request = serde_json::to_value(metadata)?;
        let Some(mut tx) = self
            .worker_transaction("model.reserve", call_id.as_str(), job_id, &request)
            .await?
        else {
            return Ok(());
        };
        let today = Utc::now().format("%Y-%m-%d").to_string();
        if tx.daily_model_calls(&metadata.resource.id, &today).await?
            >= i64::from(metadata.max_calls_per_day)
        {
            run.state = FormationState::WaitingForBudget;
            run.error = Some(Error::new(
                ErrorCode::ResourceExhausted,
                "daily model-call budget exhausted; waiting for the next UTC day",
            ));
            run.updated_at = Utc::now().to_rfc3339();
            tx.update_formation(&run).await?;
            tx.commit(json!({"state":run.state})).await?;
            return Ok(());
        }
        let mut call = ModelCall {
            id: call_id,
            job_id: job_id.into(),
            namespace_id: run.namespace_id.clone(),
            subject_id: run.subject_id.clone(),
            attempt,
            resource_id: metadata.resource.id.clone(),
            request,
            response: None,
            state: ModelCallState::Started,
            error: None,
            cost_microunits: None,
            created_at: Utc::now().to_rfc3339(),
            completed_at: None,
        };
        run.state = FormationState::Running;
        run.error = None;
        run.model_call_id = Some(call.id.clone());
        run.updated_at = call.created_at.clone();
        tx.insert_model_call(&call).await?;
        tx.update_formation(&run).await?;
        tx.commit(json!({"model_call_id":call.id})).await?;

        // The only network operation runs after its durable reservation commits.
        let execution = self.model.execute(prepared).await;
        call.completed_at = Some(execution.record.completed_at.clone());
        let record = serde_json::to_value(&execution.record)?;
        let (drafts, failure) = match execution.result {
            Ok(drafts) => (Some(drafts), None),
            Err(error) => (None, Some(error)),
        };
        call.response = Some(json!({"schema_version":1,"record":record,"memories":drafts}));
        call.state = if failure
            .as_ref()
            .is_some_and(|failure| failure.class() == RetryClass::Uncertain)
        {
            ModelCallState::Uncertain
        } else if failure.is_some() {
            ModelCallState::Failed
        } else {
            ModelCallState::Succeeded
        };
        call.error = failure.as_ref().map(|failure| failure.to_domain_error());
        let mut completed_run = run.clone();
        if let Some(error) = &failure {
            completed_run.state = if error.class() == RetryClass::Permanent || attempt >= 5 {
                FormationState::Failed
            } else {
                FormationState::Queued
            };
            completed_run.error = Some(error.to_domain_error());
        }
        completed_run.updated_at = call.completed_at.clone().expect("completed call timestamp");
        if let Some(mut tx) = self
            .worker_transaction(
                "model.complete",
                call.id.as_str(),
                job_id,
                &json!({"call_id":call.id}),
            )
            .await?
        {
            tx.update_model_call(&call).await?;
            tx.update_formation(&completed_run).await?;
            tx.commit(json!({"model_call_id":call.id,"state":call.state}))
                .await?;
        }
        if let Some(failure) = failure {
            if completed_run.state == FormationState::Failed {
                return Ok(());
            }
            let delay = failure.info.retry_after_seconds.unwrap_or(2).min(30);
            tokio::time::sleep(Duration::from_secs(delay)).await;
            return Err(failure.to_domain_error());
        }
        self.adopt_formation(&completed_run, &call).await
    }

    async fn adopt_formation(&self, run: &FormationRun, call: &ModelCall) -> Result<()> {
        let Some(mut tx) = self
            .worker_transaction(
                "memory.adopt",
                &run.job_id,
                &run.job_id,
                &json!({"job_id":run.job_id,"model_call_id":call.id}),
            )
            .await?
        else {
            return Ok(());
        };
        let current = tx.formation(&run.job_id).await?;
        if current.state == FormationState::Succeeded {
            tx.rollback().await?;
            return Ok(());
        }
        let subject = tx.subject(&run.namespace_id, &run.subject_id).await?;
        let material = tx.material(&run.namespace_id, &run.material_id).await?;
        let observation = tx
            .observation(&run.namespace_id, &run.observation_id)
            .await?;
        if subject.revision != run.subject_revision || material.content_hash != run.material_hash {
            let mut stale = current;
            stale.state = FormationState::Failed;
            stale.error = Some(Error::new(
                ErrorCode::Conflict,
                "formation input version is stale",
            ));
            stale.updated_at = Utc::now().to_rfc3339();
            tx.update_formation(&stale).await?;
            tx.commit(json!({"state":"stale"})).await?;
            return Ok(());
        }
        let saved = tx.model_call(&call.id).await?;
        if saved.state != ModelCallState::Succeeded {
            return Err(Error::new(
                ErrorCode::Conflict,
                "model output is not ready for adoption",
            ));
        }
        let response = saved
            .response
            .as_ref()
            .ok_or_else(|| Error::internal("successful call has no response"))?;
        let drafts: Vec<MemoryDraft> = serde_json::from_value(response["memories"].clone())?;
        if drafts.is_empty() || drafts.len() > 3 {
            return Err(Error::internal(
                "saved formation output has an invalid number of memories",
            ));
        }
        let formed = chrono::DateTime::parse_from_rfc3339(
            saved
                .completed_at
                .as_ref()
                .ok_or_else(|| Error::internal("successful call has no completed time"))?,
        )
        .map_err(|error| Error::internal(error.to_string()))?
        .timestamp_millis();
        let prompt_version = saved.request["prompt"]["version"]
            .as_str()
            .ok_or_else(|| Error::internal("saved prompt version missing"))?
            .to_owned();
        let mut completed = current;
        completed.memory_ids.clear();
        for draft in drafts {
            let memory = Memory {
                id: MemoryId::new(),
                namespace_id: run.namespace_id.clone(),
                subject_id: run.subject_id.clone(),
                revision: 1,
                title: draft.title,
                content: draft.content,
                epistemic_status: draft.epistemic_status,
                observation_id: run.observation_id.clone(),
                material_id: run.material_id.clone(),
                model_call_id: Some(call.id.clone()),
                prompt_version: Some(prompt_version.clone()),
                origin: MemoryOrigin::Model,
                times: CognitiveTimes {
                    event: material.event_time.clone(),
                    observation: observation.observed_at.clone(),
                    formation: TimePoint {
                        domain: "unix_ms".into(),
                        coordinate: formed,
                    },
                    validity: None,
                    recorded: utc_point(),
                },
                recorded_at: Utc::now().to_rfc3339(),
                deleted: false,
                lifecycle: MemoryLifecycle::new(formed),
                commit_seq: tx.commit_seq(),
            };
            tx.save_memory(&memory, None).await?;
            tx.enqueue(index_job(&memory)).await?;
            completed.memory_ids.push(memory.id);
        }
        completed.state = FormationState::Succeeded;
        completed.error = None;
        completed.updated_at = Utc::now().to_rfc3339();
        tx.update_formation(&completed).await?;
        tx.commit(json!({"job_id":run.job_id,"memory_ids":completed.memory_ids}))
            .await?;
        Ok(())
    }

    async fn mark_interrupted_call(&self, mut call: ModelCall) -> Result<()> {
        if let Some(mut tx) = self
            .worker_transaction(
                "model.interrupted",
                call.id.as_str(),
                &call.job_id,
                &json!({"call_id":call.id}),
            )
            .await?
        {
            call.state = ModelCallState::Uncertain;
            call.error = Some(Error::new(
                ErrorCode::DependencyUnavailable,
                "host stopped before the remote response was saved; the previous request may have completed",
            ));
            call.completed_at = Some(Utc::now().to_rfc3339());
            tx.update_model_call(&call).await?;
            tx.commit(json!({"state":call.state})).await?;
        }
        Ok(())
    }

    async fn set_formation_state(
        &self,
        run: &FormationRun,
        state: FormationState,
        error: Error,
    ) -> Result<()> {
        if run.state == state
            && run.error.as_ref().is_some_and(|previous| {
                previous.code == error.code && previous.message == error.message
            })
        {
            return Ok(());
        }
        let id = format!("{}:{}", run.job_id, ulid::Ulid::generate());
        if let Some(mut tx) = self
            .worker_transaction(
                "formation.state",
                &id,
                &run.job_id,
                &json!({"state":state,"code":error.code}),
            )
            .await?
        {
            let mut current = tx.formation(&run.job_id).await?;
            if current.state == FormationState::Succeeded {
                tx.rollback().await?;
                return Ok(());
            }
            current.state = state;
            current.error = Some(error);
            current.updated_at = Utc::now().to_rfc3339();
            tx.update_formation(&current).await?;
            tx.commit(json!({"state":current.state})).await?;
        }
        Ok(())
    }

    async fn worker_transaction(
        &self,
        kind: &str,
        operation_id: &str,
        scope: &str,
        payload: &Value,
    ) -> Result<Option<WriteTransaction>> {
        let context = CallContext {
            caller_id: "nous-worker:v1".into(),
            scope: AccessScope::Admin,
        };
        let key = operation_key(&context, kind, scope, operation_id, payload)?;
        match self.database.begin_operation(key).await? {
            BeginOperation::Replay(_) => Ok(None),
            BeginOperation::New(tx) => Ok(Some(tx)),
        }
    }
}

async fn dispatch_job(job: JobEnvelope, app: Data<App>) -> Result<()> {
    match job.kind.as_str() {
        "memory.form" => app.process_formation(&job.id).await,
        "memory.index" => app.catch_up_index().await,
        _ => Err(Error::invalid("no handler is registered for this job kind")),
    }
}
