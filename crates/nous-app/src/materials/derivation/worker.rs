use super::model;
use crate::App;
use nous_domain::{media::*, *};
use nous_foundation::media::PreparationRequest;
use serde_json::json;
use tokio_util::sync::CancellationToken;

impl App {
    pub(crate) async fn run_media_derivation(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut run = self.database.media_derivation(&job.id).await?;
        if matches!(run.job.state, JobState::Succeeded | JobState::Cancelled) {
            return Ok(());
        }
        run = match self.prepare_media_run(run, &cancel).await {
            Ok(run) => run,
            Err(error) if error.code == ErrorCode::CapabilityUnavailable => {
                let mut tx = self
                    .tasks
                    .operation("media.preparation_wait", &job.id)
                    .await?;
                let mut current = tx.job_record(&job.id).await?;
                if current.state != JobState::Cancelled {
                    self.tasks
                        .change_in(
                            &mut tx,
                            &mut current,
                            JobState::Waiting,
                            Some(WaitCondition {
                                kind: "configuration".into(),
                                requires_attention: true,
                                resume: ResumeCondition::Ready,
                                details: json!({"phase":"media_preparation"}),
                            }),
                            Some(error),
                        )
                        .await?;
                }
                tx.commit(json!({"state":current.state})).await?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if run.job.state == JobState::Cancelled {
            return Ok(());
        }
        if run.request.strategy != DerivationStrategy::DirectStructured && run.description.is_none()
        {
            let input = self.media_model_input(&run, false, &cancel).await?;
            let (namespace, subject) = run.job.subject_scope()?;
            let plan = self.model.plan(
                ConfigScope::Subject {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                },
                model::DESCRIBE,
                cancel.clone(),
            );
            let Some(call) = self
                .calls
                .invoke(
                    &job.id,
                    "describe",
                    &cancel,
                    |attempt| model::prepare(&plan, attempt, input.clone()),
                    |prepared| model::describe(&self.model, prepared),
                    None,
                )
                .await?
            else {
                return Ok(());
            };
            self.adopt_media_call(&run, &call, &input, true).await?;
            run = self.database.media_derivation(&job.id).await?;
        }
        if matches!(run.job.state, JobState::Succeeded | JobState::Cancelled) {
            return Ok(());
        }
        let include_description = run.request.strategy == DerivationStrategy::DescribeThenStructure;
        let input = self
            .media_model_input(&run, include_description, &cancel)
            .await?;
        let (namespace, subject) = run.job.subject_scope()?;
        let role = if include_description {
            model::STRUCTURE
        } else {
            model::DIRECT
        };
        let plan = self.model.plan(
            ConfigScope::Subject {
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
            },
            role,
            cancel.clone(),
        );
        let Some(call) = self
            .calls
            .invoke(
                &job.id,
                "structure",
                &cancel,
                |attempt| model::prepare(&plan, attempt, input.clone()),
                |prepared| model::structure(&self.model, prepared),
                None,
            )
            .await?
        else {
            return Ok(());
        };
        self.adopt_media_call(&run, &call, &input, false).await
    }

    pub(crate) async fn media_derivation_ready(&self, job: &JobRecord) -> bool {
        let Ok(run) = self.database.media_derivation(&job.id).await else {
            return false;
        };
        if run.prepared.is_none() {
            let Ok((namespace, subject)) = job.subject_scope() else {
                return false;
            };
            let Ok(ctx) = job.call_context("nous-media") else {
                return false;
            };
            let Ok(source) = self
                .sources
                .resolve(&ctx, namespace, subject, &run.request.source)
                .await
            else {
                return false;
            };
            return match source.payload {
                RepresentationPayload::Artifact { blob, metadata } => self
                    .media
                    .preparation_ready(&PreparationRequest {
                        blob,
                        metadata,
                        coordinate: source.coordinate,
                        delivery: run.request.delivery,
                    })
                    .await
                    .unwrap_or(false),
                _ => run.request.delivery == MediaDelivery::Native,
            };
        }
        let describe = run.request.strategy != DerivationStrategy::DirectStructured
            && run.description.is_none();
        let include_description =
            !describe && run.request.strategy == DerivationStrategy::DescribeThenStructure;
        let Ok(input) = self
            .media_model_input(&run, include_description, &CancellationToken::new())
            .await
        else {
            return false;
        };
        let (step, role) = if describe {
            ("describe", model::DESCRIBE)
        } else if include_description {
            ("structure", model::STRUCTURE)
        } else {
            ("structure", model::DIRECT)
        };
        self.calls
            .ready(
                job,
                step,
                &self.model,
                role,
                serde_json::to_value(input).ok(),
            )
            .await
    }
}
