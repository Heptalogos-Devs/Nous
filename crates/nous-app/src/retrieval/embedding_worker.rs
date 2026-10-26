//! Role-specific input validation and adoption; ModelCalls owns every external attempt.
use super::*;
use nous_foundation::models::ModelFailure;
use tokio_util::sync::CancellationToken;

impl App {
    pub(crate) async fn run_embedding_job(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        let run = self.database.embedding_run(&job.id).await?;
        if matches!(
            run.job.state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        ) {
            return Ok(());
        }
        let (namespace, subject) = run.job.subject_scope()?;
        for input in &run.inputs {
            if let Err(error) = self
                .database
                .validate_embedding_input(namespace, subject, input)
                .await
            {
                if matches!(error.code, ErrorCode::NotFound | ErrorCode::Conflict) {
                    return self.cancel_embedding(&job.id, error).await;
                }
                return Err(error);
            }
        }
        let plan = self.model.plan(
            ConfigScope::Subject {
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
            },
            super::embedding_model::ROLE,
            cancel.clone(),
        );
        let Some(call) = self
            .calls
            .invoke(
                &job.id,
                "embedding",
                &cancel,
                |attempt| {
                    let plan = &plan;
                    let run = &run;
                    async move {
                        let prepared =
                            super::embedding_model::prepare(plan, attempt, run.inputs.clone())
                                .await?;
                        if prepared.metadata.input.space.id != run.space_id
                            || prepared.metadata.input.space.producer_signature
                                != run.producer_signature
                        {
                            return Err(ModelFailure::domain(Error::new(
                                ErrorCode::Conflict,
                                "configured embedding producer differs from this accepted task",
                            )));
                        }
                        Ok(prepared)
                    }
                },
                |prepared| super::embedding_model::execute(&self.model, prepared),
            )
            .await?
        else {
            return Ok(());
        };
        self.adopt_embedding_call(&job.id, &call).await
    }

    async fn cancel_embedding(&self, id: &str, error: Error) -> Result<()> {
        let mut tx = self.tasks.operation("embedding.retire", id).await?;
        let mut job = tx.job_record(id).await?;
        if matches!(job.state, JobState::Succeeded | JobState::Cancelled) {
            tx.rollback().await?;
            return Ok(());
        }
        self.tasks
            .change_in(&mut tx, &mut job, JobState::Cancelled, None, Some(error))
            .await?;
        tx.commit(json!({"job_id":id,"state":job.state})).await?;
        Ok(())
    }

    async fn adopt_embedding_call(&self, id: &str, call: &ModelCall) -> Result<()> {
        let mut tx = self.tasks.operation("embedding.adopt", id).await?;
        let run = tx.embedding_run(id).await?;
        let mut job = run.job.clone();
        if matches!(job.state, JobState::Succeeded | JobState::Cancelled) {
            tx.rollback().await?;
            return Ok(());
        }
        let saved = tx.model_call(&call.id).await?;
        if saved.state != ModelCallState::Succeeded
            || saved.job_id != job.id
            || saved.step_key != "embedding"
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding call is unavailable for this task step",
            ));
        }
        let vectors: Vec<Vec<f32>> = serde_json::from_value(
            saved
                .response
                .as_ref()
                .ok_or_else(|| Error::internal("embedding call has no result"))?["result"]
                .clone(),
        )?;
        if vectors.len() != run.inputs.len() {
            return Err(Error::internal(
                "embedding result count differs from accepted inputs",
            ));
        }
        let (namespace, subject) = job.subject_scope()?;
        let namespace = namespace.clone();
        let subject = subject.clone();
        for input in &run.inputs {
            if let Err(error) = tx
                .validate_embedding_input(&namespace, &subject, input)
                .await
            {
                if matches!(error.code, ErrorCode::NotFound | ErrorCode::Conflict) {
                    self.tasks
                        .change_in(&mut tx, &mut job, JobState::Cancelled, None, Some(error))
                        .await?;
                    tx.commit(json!({"state":job.state})).await?;
                    return Ok(());
                }
                return Err(error);
            }
        }
        let mut promoted = 0;
        for (input, vector) in run.inputs.iter().zip(vectors) {
            let mut record = EmbeddingRecord {
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
                space_id: run.space_id.clone(),
                target: input.target.clone(),
                source_hash: input.source_hash(),
                vector,
                model_call_id: saved.id.clone(),
                commit_seq: tx.commit_seq(),
            };
            tx.save_embedding_record(&record).await?;
            if let Some(reference) = input.target.reference() {
                match tx
                    .current_embedding_input(&namespace, &subject, reference)
                    .await
                {
                    Ok(current)
                        if current.target != input.target
                            && current.source_hash() == input.source_hash() =>
                    {
                        record.target = current.target;
                        tx.save_embedding_record(&record).await?;
                        promoted += 1;
                    }
                    Ok(_)
                    | Err(Error {
                        code: ErrorCode::NotFound | ErrorCode::Conflict,
                        ..
                    }) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        self.tasks.complete_in(&mut tx,&mut job,json!({"space_id":run.space_id,"vectors":run.inputs.len(),"promoted_revisions":promoted,"model_call_id":saved.id})).await?;
        tx.commit(json!({"job_id":job.id,"state":job.state}))
            .await?;
        Ok(())
    }
}
