use super::*;
use crate::import_models as model;
use nous_foundation::models::InvocationMetadata;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimePlan {
    unit: UnitPlan,
}
pub(super) struct Temporal {
    pub call_id: Option<ModelCallId>,
    pub event: Option<TimePoint>,
    pub event_range: Option<TimeRange>,
    pub error: Option<Error>,
}
impl Temporal {
    pub fn none() -> Self {
        Self {
            call_id: None,
            event: None,
            event_range: None,
            error: None,
        }
    }
}
impl App {
    pub(super) fn register_import_time_task(&self, registry: &mut TaskRegistry) -> Result<()> {
        let owner = self.clone();
        let readiness = self.clone();
        registry.register(
            "import.time",
            move |job, cancel| {
                let app = owner.clone();
                async move { app.run_import_time(job, cancel).await }
            },
            move |job| {
                let app = readiness.clone();
                async move { app.import_time_ready(&job).await }
            },
        )
    }
    pub(super) async fn temporal_for_unit(
        &self,
        job: &JobRecord,
        plan: &UnitPlan,
        unit: &ImportTextUnit,
    ) -> Result<Option<Temporal>> {
        if !plan.llm_enabled
            || !plan.extract_time
            || unit.effective_strategy == ImportTextStrategy::Quote
        {
            return Ok(Some(Temporal::none()));
        }
        if job.dependencies.is_empty() {
            let mut tx = self.tasks.operation("import.unit.time", &job.id).await?;
            let mut current = tx.job_record(&job.id).await?;
            if current.state != JobState::Running {
                tx.rollback().await?;
                return Ok(None);
            }
            if !current.dependencies.is_empty() {
                tx.rollback().await?;
                return Ok(None);
            }
            let mut child = self.tasks.new_job(
                &tx,
                "import.time",
                job.namespace_id.clone(),
                job.subject_id.clone(),
            );
            child.checkpoint = Some(json!(TimePlan { unit: plan.clone() }));
            self.tasks
                .submit_child_in(&mut tx, &job.id, &mut child)
                .await?;
            self.tasks
                .suspend_until_in(
                    &mut tx,
                    &mut current,
                    vec![child.id],
                    DependencyPolicy::AllTerminal,
                )
                .await?;
            tx.commit(json!({"job_id":job.id})).await?;
            return Ok(None);
        }
        if job.dependencies.len() != 1 {
            return Err(Error::internal(
                "unit temporal phase has an invalid dependency catalog",
            ));
        }
        let child = self.database.job_record(&job.dependencies[0]).await?;
        if child.kind != "import.time" {
            return Err(Error::internal(
                "unit temporal phase references another task kind",
            ));
        }
        if !matches!(
            child.state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        ) {
            return Ok(None);
        }
        let call = self.database.latest_model_call(&child.id, "time").await?;
        let call_id = call.as_ref().map(|call| call.id.clone());
        if child.state != JobState::Succeeded {
            return Ok(Some(Temporal {
                call_id,
                event: None,
                event_range: None,
                error: Some(
                    child
                        .error
                        .or_else(|| call.and_then(|call| call.error))
                        .unwrap_or_else(|| {
                            Error::new(
                                ErrorCode::DependencyUnavailable,
                                "temporal child did not succeed",
                            )
                        }),
                ),
            }));
        }
        let call =
            call.ok_or_else(|| Error::internal("successful temporal task has no model call"))?;
        let raw: model::TimeExtraction = models::output(&call)?;
        Ok(Some(match compile::temporal(&raw) {
            Ok((event, event_range)) => Temporal {
                call_id,
                event,
                event_range,
                error: None,
            },
            Err(error) => Temporal {
                call_id,
                event: None,
                event_range: None,
                error: Some(error),
            },
        }))
    }
    async fn run_import_time(&self, job: JobRecord, cancel: CancellationToken) -> Result<()> {
        let plan: TimePlan = serde_json::from_value(
            job.checkpoint
                .clone()
                .ok_or_else(|| Error::internal("temporal job has no frozen unit"))?,
        )?;
        let (_, semantic, reference_time) = models::input(self, &job, &plan.unit, &cancel).await?;
        let input = model::ImportTimeInput {
            namespace_id: semantic.namespace_id,
            subject_id: semantic.subject_id,
            unit_id: semantic.unit_id,
            source_refs: semantic.source_refs,
            text: semantic.text,
            context: semantic.context,
            reference_time,
        };
        let (ns, subject) = job.subject_scope()?;
        let invocation = self.model.plan(
            ConfigScope::Subject {
                namespace_id: ns.clone(),
                subject_id: subject.clone(),
            },
            model::TIME,
            cancel.clone(),
        );
        let call=self.calls.invoke::<InvocationMetadata<model::ImportTimeInput>,model::TimeExtraction,_,_,_,_>(&job.id,"time",&cancel,|attempt|model::prepare_time(&invocation,attempt,input.clone()),|prepared|model::time(&self.model,prepared),None).await?;
        let Some(call) = call else {
            return Ok(());
        };
        let mut tx = self
            .tasks
            .operation("import.time.complete", &job.id)
            .await?;
        let mut current = tx.job_record(&job.id).await?;
        if current.state != JobState::Running {
            tx.rollback().await?;
            return Ok(());
        }
        self.tasks
            .complete_in(&mut tx, &mut current, json!({"model_call_id":call.id}))
            .await?;
        tx.commit(json!({"job_id":job.id})).await?;
        Ok(())
    }
    async fn import_time_ready(&self, job: &JobRecord) -> bool {
        let Ok(plan) =
            serde_json::from_value::<TimePlan>(job.checkpoint.clone().unwrap_or_default())
        else {
            return false;
        };
        let Ok((_, semantic, reference_time)) =
            models::input(self, job, &plan.unit, &CancellationToken::new()).await
        else {
            return true;
        };
        self.calls
            .ready(
                job,
                "time",
                &self.model,
                model::TIME,
                Some(json!(model::ImportTimeInput {
                    namespace_id: semantic.namespace_id,
                    subject_id: semantic.subject_id,
                    unit_id: semantic.unit_id,
                    source_refs: semantic.source_refs,
                    text: semantic.text,
                    context: semantic.context,
                    reference_time
                })),
            )
            .await
    }
}
