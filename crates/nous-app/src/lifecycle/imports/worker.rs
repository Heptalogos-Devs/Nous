use super::*;
use nous_foundation::tasks::DependencyResolution;
use std::collections::BTreeSet;

impl App {
    pub(crate) fn register_import_tasks(
        &self,
        registry: &mut nous_foundation::tasks::TaskRegistry,
    ) -> Result<()> {
        let owner = self.clone();
        registry.register(
            "import.text",
            move |job, cancel| {
                let owner = owner.clone();
                async move { owner.run_import_job(job, cancel).await }
            },
            |_| async { true },
        )?;
        self.register_import_processing_tasks(registry)
    }
    async fn run_import_job(&self, job: JobRecord, cancel: CancellationToken) -> Result<()> {
        let plan: ImportPlan = serde_json::from_value(
            job.checkpoint
                .clone()
                .ok_or_else(|| Error::internal("import task has no frozen input catalog"))?,
        )?;
        loop {
            let current = self.database.job_record(&job.id).await?;
            if current.state != JobState::Running {
                return Ok(());
            }
            if cancel.is_cancelled() {
                return self.pause_import(&job.id).await;
            }
            let result = progress(&current)?;
            let Some(parsed) = plan.records.get(result.items.len()) else {
                return self.complete_import(&job.id, &plan).await;
            };
            if let Some(error) = &parsed.error {
                self.import_progress(&job, &plan, parsed, None, Some(error.clone()))
                    .await?;
                continue;
            }
            let record = parsed.record.clone().ok_or_else(|| {
                Error::internal("import catalog entry has neither record nor rejection")
            })?;
            match self
                .accept_import_record(&job, &plan, parsed, record, &cancel)
                .await
            {
                Ok(()) => {}
                Err(error) if error.code == ErrorCode::Cancelled && cancel.is_cancelled() => {
                    return self.pause_import(&job.id).await;
                }
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::Internal | ErrorCode::DependencyUnavailable
                    ) =>
                {
                    return Err(error);
                }
                Err(error) => {
                    self.import_progress(&job, &plan, parsed, None, Some(error))
                        .await?
                }
            }
        }
    }
    async fn complete_import(&self, id: &str, plan: &ImportPlan) -> Result<()> {
        let mut tx = self.tasks.operation("import.complete", id).await?;
        let Some(mut current) = current_for(&mut tx, id, plan.records.len()).await? else {
            tx.rollback().await?;
            return Ok(());
        };
        let result = progress(&current)?;
        if plan.completion == ImportCompletion::Formed {
            let mut dependencies = BTreeSet::new();
            for admission in result
                .items
                .iter()
                .filter_map(|item| item.admission.as_ref())
            {
                dependencies.insert(admission.formation_job_id.clone().ok_or_else(|| {
                    Error::internal("formed import admission has no actual formation task")
                })?);
            }
            match self
                .tasks
                .suspend_until_in(
                    &mut tx,
                    &mut current,
                    dependencies.into_iter().collect(),
                    DependencyPolicy::AllSucceeded,
                )
                .await?
            {
                DependencyResolution::Ready => {}
                DependencyResolution::Suspended | DependencyResolution::Failed => {
                    tx.commit(json!({"job_id":id,"state":current.state,"dependencies":current.dependencies})).await?;
                    return Ok(());
                }
            }
        }
        self.tasks
            .complete_in(&mut tx, &mut current, json!(result))
            .await?;
        tx.commit(json!({"job_id":id,"state":current.state}))
            .await?;
        Ok(())
    }
    async fn pause_import(&self, id: &str) -> Result<()> {
        let mut tx = self.tasks.operation("import.interrupted", id).await?;
        let mut job = tx.job_record(id).await?;
        if job.state != JobState::Running {
            tx.rollback().await?;
            return Ok(());
        }
        self.tasks
            .change_in(
                &mut tx,
                &mut job,
                JobState::Waiting,
                Some(WaitCondition {
                    kind: "interrupted".into(),
                    requires_attention: false,
                    resume: ResumeCondition::Ready,
                    details: json!({}),
                }),
                None,
            )
            .await?;
        tx.commit(json!({"job_id":id,"state":job.state})).await?;
        Ok(())
    }
}
