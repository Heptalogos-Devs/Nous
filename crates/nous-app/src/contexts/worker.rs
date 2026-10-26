use super::*;
use tokio_util::sync::CancellationToken;

impl App {
    pub(crate) async fn run_context_preparation(
        &self,
        job: JobRecord,
        _cancel: CancellationToken,
    ) -> Result<()> {
        let mut preparation = self.database.context_preparation_by_job(&job.id).await?;
        if preparation.response.is_some() {
            return Ok(());
        }
        let policy = preparation
            .policy
            .clone()
            .ok_or_else(|| Error::internal("context policy snapshot missing"))?;
        let input = model::input(&preparation);
        let mut proposal = None;
        let mut call_id = None;
        if policy.steward != ContextStewardMode::Off {
            if let Some((previous, id)) =
                self.reusable_context_steward(&preparation, &input).await?
            {
                proposal = Some(previous);
                call_id = Some(id);
            }
        }
        if proposal.is_none()
            && policy.steward != ContextStewardMode::Off
            && !input.sources.is_empty()
        {
            if let Some(child) = &preparation.steward_job_id {
                let child = self.database.job_record(child).await?;
                match child.state {
                    JobState::Succeeded => {
                        let result = child
                            .result
                            .as_ref()
                            .ok_or_else(|| Error::internal("Steward result is missing"))?;
                        let value: model::StewardProposal =
                            serde_json::from_value(result["proposal"].clone())?;
                        value.validate(&input)?;
                        proposal = Some(value);
                        call_id = Some(serde_json::from_value(result["call_id"].clone())?);
                    }
                    JobState::Failed | JobState::Cancelled
                        if policy.steward == ContextStewardMode::Optional =>
                    {
                        preparation
                            .degradation
                            .push("optional_steward_failed".into())
                    }
                    JobState::Waiting if policy.steward == ContextStewardMode::Optional => {
                        preparation
                            .degradation
                            .push("optional_steward_waiting".into());
                    }
                    JobState::Failed | JobState::Cancelled => {
                        return Err(child.error.unwrap_or_else(|| {
                            Error::new(
                                ErrorCode::DependencyUnavailable,
                                "required Steward did not complete",
                            )
                        }));
                    }
                    _ => {
                        self.wait_for_context_steward(&job, &child.id).await?;
                        return Ok(());
                    }
                }
            } else if policy.steward == ContextStewardMode::Optional
                && !self
                    .calls
                    .ready(
                        &job,
                        "steward",
                        &self.model,
                        model::ROLE,
                        Some(serde_json::to_value(&input)?),
                    )
                    .await
            {
                preparation
                    .degradation
                    .push("optional_steward_unavailable".into());
            } else {
                let mut tx = self
                    .tasks
                    .operation("context.steward_submit", &job.id)
                    .await?;
                let mut current = tx.context_preparation(&preparation.id).await?;
                if current.job.state == JobState::Cancelled {
                    tx.rollback().await?;
                    return Ok(());
                }
                let (ns, subject) = job.subject_scope()?;
                let child = self.tasks.new_job(
                    &tx,
                    "context.steward",
                    Some(ns.clone()),
                    Some(subject.clone()),
                );
                self.tasks.submit_in(&mut tx, &child).await?;
                tx.insert_context_steward(&current.id, &child).await?;
                current.steward_job_id = Some(child.id.clone());
                tx.update_context_preparation(&current).await?;
                self.tasks
                    .change_in(
                        &mut tx,
                        &mut current.job,
                        JobState::Waiting,
                        Some(WaitCondition {
                            kind: "context_steward".into(),
                            resume: ResumeCondition::Ready,
                            details: json!({"job_id":child.id}),
                        }),
                        None,
                    )
                    .await?;
                tx.commit(json!({"steward_job_id":child.id})).await?;
                return Ok(());
            }
        }
        let segments = compile::segments(&mut preparation, proposal.as_ref(), call_id.as_ref())?;
        self.finish_context_projection(&job, preparation, segments, call_id)
            .await
    }
    async fn wait_for_context_steward(&self, job: &JobRecord, child: &str) -> Result<()> {
        let mut tx = self
            .tasks
            .operation("context.steward_wait", &job.id)
            .await?;
        let mut current = tx.job_record(&job.id).await?;
        if current.state != JobState::Cancelled {
            self.tasks
                .change_in(
                    &mut tx,
                    &mut current,
                    JobState::Waiting,
                    Some(WaitCondition {
                        kind: "context_steward".into(),
                        resume: ResumeCondition::Ready,
                        details: json!({"job_id":child}),
                    }),
                    None,
                )
                .await?;
        }
        tx.commit(json!({"state":current.state})).await?;
        Ok(())
    }
    async fn finish_context_projection(
        &self,
        job: &JobRecord,
        mut preparation: ContextPreparation,
        segments: Vec<ContextSegment>,
        call: Option<ModelCallId>,
    ) -> Result<()> {
        let Some(mut tx) = self
            .worker_transaction(
                job,
                "context.project",
                &job.id,
                &job.id,
                &json!({"preparation_id":preparation.id}),
            )
            .await?
        else {
            return Ok(());
        };
        let current = tx.context_preparation(&preparation.id).await?;
        if current.job.state == JobState::Cancelled || current.response.is_some() {
            tx.rollback().await?;
            return Ok(());
        }
        let (ns, subject) = job.subject_scope()?;
        if call.is_none()
            && preparation
                .policy
                .as_ref()
                .is_some_and(|policy| policy.steward == ContextStewardMode::Optional)
        {
            if let Some(child) = &preparation.steward_job_id {
                let mut child = tx.job_record(child).await?;
                self.tasks.cancel_in(&mut tx, &mut child, None).await?;
            }
        }
        for segment in &segments {
            tx.validate_context_targets(ns, subject, &segment.sources)
                .await?;
        }
        let id = ContextProjectionId::new();
        let epistemic_view = compile::epistemic_view(&segments, &preparation.degradation);
        let mut projection = ContextProjection {
            source_read: job.source_read.clone(),
            id: id.clone(),
            namespace_id: ns.clone(),
            subject_id: subject.clone(),
            session_id: preparation.request.session_id.clone(),
            consumer_id: preparation.request.consumer_id.clone(),
            instance_id: preparation.request.instance_id.clone(),
            usage: preparation.request.usage.clone(),
            config_digest: preparation
                .config_digest
                .clone()
                .ok_or_else(|| Error::internal("context digest missing"))?,
            session_revision: preparation
                .session_revision
                .ok_or_else(|| Error::internal("session snapshot missing"))?,
            work_context: preparation.work_context.clone(),
            source_watermark: preparation.source_watermark.unwrap_or_default(),
            segments,
            epistemic_view,
            cursor: ContextCursor {
                track_id: String::new(),
                epoch_id: String::new(),
                revision: 0,
                projection_id: id,
            },
            steward_call: call,
            degradation: preparation.degradation.clone(),
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        let previous = if let Some(cursor) = &preparation.request.known_cursor {
            match tx.context_projection(&cursor.projection_id).await {
                Ok(previous) => Some(previous),
                Err(error) if error.code == ErrorCode::NotFound => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let response = compile::response(&preparation, projection.clone(), previous)?;
        projection.cursor = response.cursor.clone();
        tx.save_context_projection(&preparation.id, &projection)
            .await?;
        preparation.response = Some(response);
        preparation.job = current.job;
        tx.update_context_preparation(&preparation).await?;
        self.tasks
            .complete_in(
                &mut tx,
                &mut preparation.job,
                json!({"preparation_id":preparation.id,"projection_id":projection.id}),
            )
            .await?;
        tx.commit(json!({"preparation_id":preparation.id,"projection_id":projection.id}))
            .await?;
        Ok(())
    }
    pub(crate) async fn context_preparation_ready(&self, job: &JobRecord) -> bool {
        let Ok(preparation) = self.database.context_preparation_by_job(&job.id).await else {
            return false;
        };
        let Some(child) = &preparation.steward_job_id else {
            return true;
        };
        let Ok(child) = self.database.job_record(child).await else {
            return false;
        };
        matches!(
            child.state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        ) || (child.state == JobState::Waiting
            && preparation
                .policy
                .as_ref()
                .is_some_and(|policy| policy.steward == ContextStewardMode::Optional))
    }
    pub(crate) async fn run_context_steward(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        let preparation = self.database.context_preparation_by_job(&job.id).await?;
        if matches!(
            preparation.job.state,
            JobState::Cancelled | JobState::Failed | JobState::Succeeded
        ) || preparation.response.is_some()
        {
            self.tasks
                .cancel(
                    &job.call_context("context")?,
                    &format!("{}:no_demand", job.id),
                    &job.id,
                )
                .await?;
            return Ok(());
        }
        let (ns, subject) = job.subject_scope()?;
        let input = model::input(&preparation);
        let admission = super::admission::StewardAdmission(preparation.id.clone());
        let plan = self.model.plan(
            ConfigScope::Subject {
                namespace_id: ns.clone(),
                subject_id: subject.clone(),
            },
            model::ROLE,
            cancel.clone(),
        );
        let Some(call) = self
            .calls
            .invoke(
                &job.id,
                "steward",
                &cancel,
                |attempt| model::prepare(&plan, attempt, input.clone()),
                |prepared| model::execute(&self.model, prepared),
                Some(&admission),
            )
            .await?
        else {
            return Ok(());
        };
        let Some(mut tx) = self
            .worker_transaction(
                &job,
                "context.steward_result",
                &job.id,
                &job.id,
                &json!({"call_id":call.id}),
            )
            .await?
        else {
            return Ok(());
        };
        let mut current = tx.job_record(&job.id).await?;
        if current.state == JobState::Cancelled {
            tx.rollback().await?;
            return Ok(());
        }
        let proposal: model::StewardProposal = serde_json::from_value(
            call.response
                .as_ref()
                .ok_or_else(|| Error::internal("successful Steward call has no result"))?["result"]
                .clone(),
        )?;
        proposal.validate(&input)?;
        self.tasks
            .complete_in(
                &mut tx,
                &mut current,
                json!({"call_id":call.id,"proposal":proposal}),
            )
            .await?;
        tx.commit(json!({"call_id":call.id})).await?;
        Ok(())
    }
    pub(crate) async fn context_steward_ready(&self, job: &JobRecord) -> bool {
        let Ok(preparation) = self.database.context_preparation_by_job(&job.id).await else {
            return false;
        };
        if matches!(
            preparation.job.state,
            JobState::Cancelled | JobState::Failed | JobState::Succeeded
        ) || preparation.response.is_some()
        {
            return true;
        }
        self.calls
            .ready(
                job,
                "steward",
                &self.model,
                model::ROLE,
                serde_json::to_value(model::input(&preparation)).ok(),
            )
            .await
    }
}
