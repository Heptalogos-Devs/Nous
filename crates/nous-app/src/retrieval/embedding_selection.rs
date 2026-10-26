//! Content rights are selected from live subscriptions before every actual attempt.
use super::*;
impl App {
    pub(super) async fn select_embedding_inputs(
        &self,
        id: &str,
    ) -> Result<Vec<EmbeddingInputItem>> {
        let mut tx = self.tasks.operation("embedding.select", id).await?;
        let run = tx.embedding_run(id).await?;
        let mut job = run.job.clone();
        if matches!(
            job.state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        ) {
            tx.rollback().await?;
            return Ok(vec![]);
        }
        let (namespace, subject) = job.subject_scope()?;
        let namespace = namespace.clone();
        let subject = subject.clone();
        let owned = tx.claimed_embedding_hashes(id).await?;
        let mut inputs = Vec::new();
        for hash in &run.content_hashes {
            if !owned.contains(hash) {
                continue;
            }
            let subscriptions = tx
                .embedding_subscriptions(&namespace, &subject, &run.space_id, hash)
                .await?;
            let mut representative = None;
            for subscription in subscriptions {
                match tx.validate_embedding_subscription(&subscription).await {
                    Ok(()) => {
                        if representative.is_none() {
                            representative = Some(subscription.input);
                        }
                    }
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::NotFound | ErrorCode::Conflict | ErrorCode::Forbidden
                        ) =>
                    {
                        tx.unsubscribe_embedding(&subscription).await?
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some(input) = representative {
                inputs.push(input);
            } else {
                tx.release_embedding_claim(id, hash).await?;
            }
        }
        if inputs.is_empty() && tx.model_step_calls(id, "embedding").await?.is_empty() {
            self.tasks
                .change_in(
                    &mut tx,
                    &mut job,
                    JobState::Cancelled,
                    None,
                    Some(Error::new(
                        ErrorCode::Cancelled,
                        "embedding content has no remaining consumers",
                    )),
                )
                .await?;
        }
        tx.commit(json!({"job_id":id,"selected_content":inputs.iter().map(EmbeddingInputItem::source_hash).collect::<Vec<_>>()})).await?;
        Ok(inputs)
    }
}
