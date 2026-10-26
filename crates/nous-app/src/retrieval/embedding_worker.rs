//! Actual content calls and independent target subscriptions meet only during adoption.
use super::*;
use nous_foundation::{
    model_calls::{AdmissionDecision, AdmissionFuture, CallAdmission},
    models::{InvocationMetadata, ModelFailure},
};
use tokio_util::sync::CancellationToken;
type EmbeddingMetadata = InvocationMetadata<super::embedding_model::EmbeddingInput>;
struct EmbeddingAdmission {
    job_id: String,
}
impl CallAdmission<EmbeddingMetadata> for EmbeddingAdmission {
    fn validate<'a>(
        &'a self,
        tx: &'a mut nous_storage::WriteTransaction,
        metadata: &'a EmbeddingMetadata,
    ) -> AdmissionFuture<'a> {
        Box::pin(async move {
            match tx
                .validate_embedding_reservation(&self.job_id, &metadata.input.inputs)
                .await
            {
                Ok(()) => Ok(AdmissionDecision::Ready),
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::NotFound | ErrorCode::Conflict | ErrorCode::Forbidden
                    ) =>
                {
                    Ok(AdmissionDecision::Refresh)
                }
                Err(error) => Err(error),
            }
        })
    }
}
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
        let plan = self.model.plan(
            ConfigScope::Subject {
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
            },
            super::embedding_model::ROLE,
            cancel.clone(),
        );
        let guard = EmbeddingAdmission {
            job_id: job.id.clone(),
        };
        let Some(call) = self
            .calls
            .invoke(
                &job.id,
                "embedding",
                &cancel,
                |attempt| {
                    let plan = &plan;
                    let run = &run;
                    let owner = self;
                    async move {
                        let inputs = owner
                            .select_embedding_inputs(&run.job.id)
                            .await
                            .map_err(ModelFailure::domain)?;
                        if inputs.is_empty() {
                            return Err(ModelFailure::domain(Error::new(
                                ErrorCode::Cancelled,
                                "embedding content has no live consumers",
                            )));
                        }
                        if plan
                            .resolve(&attempt)
                            .await?
                            .space()
                            .map_err(ModelFailure::domain)?
                            .id
                            != run.space_id
                        {
                            return Err(ModelFailure::domain(Error::new(
                                ErrorCode::Conflict,
                                "configured embedding space differs from this accepted work",
                            )));
                        }
                        let prepared =
                            super::embedding_model::prepare(plan, attempt, inputs).await?;
                        if prepared
                            .metadata
                            .embedding_space
                            .as_ref()
                            .map(|space| space.id.as_str())
                            != Some(run.space_id.as_str())
                        {
                            return Err(ModelFailure::domain(Error::new(
                                ErrorCode::Conflict,
                                "actual embedding space differs from this accepted work",
                            )));
                        }
                        Ok(prepared)
                    }
                },
                |prepared| super::embedding_model::execute(&self.model, prepared),
                Some(&guard),
            )
            .await?
        else {
            return Ok(());
        };
        self.adopt_embedding_call(&job.id, &call).await
    }
    async fn adopt_embedding_call(&self, id: &str, call: &ModelCall) -> Result<()> {
        let mut tx = self.tasks.operation("embedding.adopt", id).await?;
        let run = tx.embedding_run(id).await?;
        let mut job = run.job.clone();
        if matches!(
            job.state,
            JobState::Succeeded | JobState::Cancelled | JobState::Failed
        ) {
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
                "embedding call is unavailable for this work step",
            ));
        }
        let space: EmbeddingSpace = serde_json::from_value(
            saved
                .request
                .get("embedding_space")
                .cloned()
                .ok_or_else(|| Error::invalid("embedding call has no actual space"))?,
        )?;
        let producer: EmbeddingProducer = serde_json::from_value(
            saved
                .request
                .get("embedding_producer")
                .cloned()
                .ok_or_else(|| Error::invalid("embedding call has no actual producer"))?,
        )?;
        if space.id != run.space_id
            || producer.space_id != run.space_id
            || producer.resource_id != saved.resource_id
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "actual embedding producer differs from accepted work",
            ));
        }
        let contents = nous_storage::embedding_call_contents(&saved)?;
        if contents
            .iter()
            .any(|content| !run.content_hashes.contains(&content.source_hash))
        {
            return Err(Error::invalid("embedding call contains unaccepted content"));
        }
        let (namespace, subject) = job.subject_scope()?;
        let namespace = namespace.clone();
        let subject = subject.clone();
        let mut adopted = BTreeSet::new();
        let mut retired = 0;
        for content in &contents {
            let Some(vector) = &content.vector else {
                continue;
            };
            let accepted = tx
                .save_embedding_content(&EmbeddingContent {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                    space_id: run.space_id.clone(),
                    source_hash: content.source_hash.clone(),
                    vector: vector.clone(),
                    model_call_id: saved.id.clone(),
                    commit_seq: tx.commit_seq(),
                })
                .await?;
            let subscriptions = tx
                .embedding_subscriptions(&namespace, &subject, &run.space_id, &content.source_hash)
                .await?;
            for subscription in subscriptions {
                let input = &subscription.input;
                match tx.validate_embedding_subscription(&subscription).await {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::NotFound | ErrorCode::Conflict | ErrorCode::Forbidden
                        ) =>
                    {
                        tx.unsubscribe_embedding(&subscription).await?;
                        retired += 1;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
                let record = EmbeddingRecord {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                    space_id: run.space_id.clone(),
                    target: input.target.clone(),
                    source_hash: content.source_hash.clone(),
                    vector: accepted.vector.clone(),
                    model_call_id: accepted.model_call_id.clone(),
                    commit_seq: tx.commit_seq(),
                };
                tx.save_embedding_record(&record, input).await?;
                adopted.insert(input.source_key()?);
            }
        }
        for hash in &run.content_hashes {
            tx.release_embedding_claim(id, hash).await?;
        }
        self.tasks.complete_in(&mut tx,&mut job,json!({"space_id":run.space_id,"remote_contents":contents.len(),"targets":adopted.len(),"retired_subscriptions":retired,"model_call_id":saved.id})).await?;
        tx.commit(json!({"job_id":job.id,"state":job.state}))
            .await?;
        Ok(())
    }
}
