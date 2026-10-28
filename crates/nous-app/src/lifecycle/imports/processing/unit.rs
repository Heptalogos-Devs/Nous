use super::*;
use crate::import_models as model;

impl App {
    pub(super) async fn run_import_unit(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        if job.result.is_some() {
            return self.complete_import_unit_vectors(&job).await;
        }
        let plan: UnitPlan = serde_json::from_value(
            job.checkpoint
                .clone()
                .ok_or_else(|| Error::internal("unit task has no accepted plan"))?,
        )?;
        let (unit, input, _) = models::input(self, &job, &plan, &cancel).await?;
        if self.database.job_record(&job.id).await?.state != JobState::Running {
            return Ok(());
        }
        let Some(semantic) =
            models::semantic(self, &job, &unit, input, plan.llm_enabled, &cancel).await?
        else {
            return Ok(());
        };
        let current = self.database.job_record(&job.id).await?;
        if current.state != JobState::Running {
            return Ok(());
        }
        let Some(temporal) = self.temporal_for_unit(&current, &plan, &unit).await? else {
            return Ok(());
        };
        self.adopt_import_unit(&current, &plan, &unit, semantic, temporal)
            .await
    }
    pub(super) async fn import_unit_ready(&self, job: &JobRecord) -> bool {
        if job.result.is_some() {
            return true;
        }
        let Ok(plan) =
            serde_json::from_value::<UnitPlan>(job.checkpoint.clone().unwrap_or_default())
        else {
            return false;
        };
        if plan.vectors == ImportVectorPolicy::Required
            && job.wait.as_ref().is_some_and(|wait| {
                wait.details
                    .get("resource")
                    .and_then(serde_json::Value::as_str)
                    == Some("import_vectors")
            })
        {
            let Ok((ns, subject)) = job.subject_scope() else {
                return false;
            };
            return self
                .embedding_configuration(ns, subject)
                .await
                .and_then(|config| config.space())
                .is_ok();
        }
        let Ok((unit, input, _)) = models::input(self, job, &plan, &CancellationToken::new()).await
        else {
            return true;
        };
        if !plan.llm_enabled || unit.effective_strategy == ImportTextStrategy::Quote {
            return true;
        }
        let role = match unit.effective_strategy {
            ImportTextStrategy::Factual => model::FACTUAL,
            ImportTextStrategy::Narrative | ImportTextStrategy::ChatLog => model::NARRATIVE,
            _ => return true,
        };
        self.calls
            .ready(job, "semantic", &self.model, role, Some(json!(input)))
            .await
    }
    async fn complete_import_unit_vectors(&self, job: &JobRecord) -> Result<()> {
        let result: ImportUnitProcessResult = serde_json::from_value(
            job.result
                .clone()
                .ok_or_else(|| Error::internal("unit has no adopted output"))?,
        )?;
        let mut tx = self
            .tasks
            .operation("import.unit.index.complete", &job.id)
            .await?;
        let mut current = tx.job_record(&job.id).await?;
        if current.state != JobState::Running {
            tx.rollback().await?;
            return Ok(());
        }
        if self
            .tasks
            .suspend_until_in(
                &mut tx,
                &mut current,
                result.vector_job_ids.clone(),
                DependencyPolicy::AllSucceeded,
            )
            .await?
            == DependencyResolution::Ready
        {
            self.tasks
                .complete_in(&mut tx, &mut current, json!(result))
                .await?;
        }
        tx.commit(json!({"job_id":job.id})).await?;
        Ok(())
    }
}
