//! Immutable target subscriptions retain each accepting operation's authority.
use super::target_row;
use crate::{WriteTransaction, storage_error, write_error};
use nous_domain::*;
use sqlx::Row;
impl WriteTransaction {
    pub async fn subscribe_embedding(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        input: &EmbeddingInputItem,
    ) -> Result<()> {
        self.context().require_subject(namespace, subject)?;
        let context = self.context().clone();
        self.require_embedding_source_read(&context, namespace, subject, input)
            .await?;
        self.embedding_space(space).await?;
        self.validate_embedding_input(namespace, subject, input)
            .await?;
        let hash = input.source_hash();
        sqlx::query("INSERT INTO embedding_subscriptions(namespace_id,subject_id,space_id,source_hash,target,source_key,identity_key,parts,read_requirements,accepted_commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT DO NOTHING")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space).bind(&hash).bind(serde_json::to_string(&input.target)?).bind(input.source_key()?).bind(input.target.identity_key(&hash)?).bind(serde_json::to_string(&input.parts)?).bind(serde_json::to_string(&input.read_requirements)?).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub async fn embedding_subscriptions(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        hash: &str,
    ) -> Result<Vec<EmbeddingSubscription>> {
        sqlx::query("SELECT * FROM embedding_subscriptions WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=? AND retired_commit_seq IS NULL ORDER BY accepted_commit_seq,source_key")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space).bind(hash).fetch_all(&mut *self.tx).await.map_err(storage_error)?.into_iter().map(|row|Ok(EmbeddingSubscription{
                namespace_id:NamespaceId(row.get("namespace_id")),subject_id:SubjectId(row.get("subject_id")),space_id:row.get("space_id"),accepted_commit_seq:row.get("accepted_commit_seq"),input:EmbeddingInputItem{target:target_row(&row)?,parts:serde_json::from_str(row.get("parts"))?,read_requirements:serde_json::from_str(row.get("read_requirements"))?},
            })).collect()
    }
    pub async fn require_embedding_source_read(
        &mut self,
        context: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        input: &EmbeddingInputItem,
    ) -> Result<()> {
        self.require_source_requirements(context, namespace, subject, &input.read_requirements)
            .await?;
        if let Some(target) = input.target.item() {
            self.require_source_read(context, namespace, subject, target)
                .await?;
        }
        for part in &input.parts {
            let reference = match part {
                EmbeddingPart::Text { source, .. } => source.as_ref(),
                EmbeddingPart::Media { source, .. } => Some(source),
            };
            if let Some(reference) = reference {
                self.require_source_read(
                    context,
                    namespace,
                    subject,
                    &RetrievalTarget::Media(reference.clone()),
                )
                .await?;
            }
        }
        Ok(())
    }
    pub async fn validate_embedding_subscription(
        &mut self,
        subscription: &EmbeddingSubscription,
    ) -> Result<()> {
        let context = self
            .accepted_context(subscription.accepted_commit_seq)
            .await?;
        self.require_embedding_source_read(
            &context,
            &subscription.namespace_id,
            &subscription.subject_id,
            &subscription.input,
        )
        .await?;
        self.validate_embedding_input(
            &subscription.namespace_id,
            &subscription.subject_id,
            &subscription.input,
        )
        .await
    }
    pub async fn unsubscribe_embedding(
        &mut self,
        subscription: &EmbeddingSubscription,
    ) -> Result<()> {
        sqlx::query("UPDATE embedding_subscriptions SET retired_commit_seq=? WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=? AND source_key=? AND accepted_commit_seq=? AND retired_commit_seq IS NULL")
            .bind(self.commit_seq()).bind(subscription.namespace_id.as_str()).bind(subscription.subject_id.as_str()).bind(&subscription.space_id).bind(subscription.input.source_hash()).bind(subscription.input.source_key()?).bind(subscription.accepted_commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub async fn validate_embedding_reservation(
        &mut self,
        job: &str,
        inputs: &[EmbeddingInputItem],
    ) -> Result<()> {
        let run = self.embedding_run(job).await?;
        let (namespace, subject) = run.job.subject_scope()?;
        let claims = self.claimed_embedding_hashes(job).await?;
        let mut hashes = std::collections::HashSet::new();
        if inputs.is_empty() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding work has no live content consumers",
            ));
        }
        for input in inputs {
            let hash = input.source_hash();
            if !hashes.insert(hash.clone())
                || !run.content_hashes.contains(&hash)
                || !claims.contains(&hash)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding content claim changed before reservation",
                ));
            }
            let accepted = self
                .embedding_subscriptions(namespace, subject, &run.space_id, &hash)
                .await?;
            let mut representative = false;
            for subscription in accepted
                .iter()
                .filter(|subscription| subscription.input == *input)
            {
                match self.validate_embedding_subscription(subscription).await {
                    Ok(()) => {
                        representative = true;
                        break;
                    }
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::Forbidden | ErrorCode::NotFound | ErrorCode::Conflict
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
            if !representative {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding representative subscription changed before reservation",
                ));
            }
        }
        Ok(())
    }
}
