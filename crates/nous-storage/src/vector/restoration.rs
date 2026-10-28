//! Reattach only already accepted mathematical content; restoration never queues a model call.
use crate::{WriteTransaction, storage_error};
use nous_domain::*;

impl WriteTransaction {
    pub async fn reuse_retained_cognitive_vectors(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &CognitiveRef,
    ) -> Result<usize> {
        let text = super::source::text(&mut self.tx, namespace, subject, reference).await?;
        let input = EmbeddingInputItem::new(
            EmbeddingTarget::Item(RetrievalTarget::Cognition(reference.clone())),
            vec![EmbeddingPart::Text { text, source: None }],
        )?;
        let context = self.context().clone();
        match self
            .require_embedding_source_read(&context, namespace, subject, &input)
            .await
        {
            Ok(()) => {}
            Err(error) if error.code == ErrorCode::Forbidden => return Ok(0),
            Err(error) => return Err(error),
        }
        let spaces:Vec<String>=sqlx::query_scalar("SELECT space_id FROM embedding_contents WHERE namespace_id=? AND subject_id=? AND source_hash=? ORDER BY space_id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(input.source_hash()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut reused = 0;
        for space in spaces {
            let content = self
                .matching_embedding(namespace, subject, &space, &input.source_hash())
                .await?
                .ok_or_else(crate::not_found)?;
            self.subscribe_embedding(namespace, subject, &space, &input)
                .await?;
            self.save_embedding_record(
                &EmbeddingRecord {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                    space_id: space,
                    source_hash: input.source_hash(),
                    target: input.target.clone(),
                    vector: content.vector,
                    model_call_id: content.model_call_id,
                    commit_seq: self.commit_seq(),
                },
                &input,
            )
            .await?;
            reused += 1;
        }
        Ok(reused)
    }
}
