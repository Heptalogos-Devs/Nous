use crate::WriteTransaction;
use nous_domain::{media::MediaReference, *};

impl WriteTransaction {
    /// Exact sources are checked under the same write transaction as a model reservation.
    pub async fn require_query_target(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        target: &RetrievalTarget,
        current_head: bool,
    ) -> Result<()> {
        let ctx = self.context().clone();
        ctx.require_subject(namespace, subject)?;
        self.require_source_requirements(
            &ctx,
            namespace,
            subject,
            &[SourceReadRef::target(target)],
        )
        .await?;
        if !current_head {
            return Ok(());
        }
        if !self
            .bundle_target_active(namespace, subject, target)
            .await?
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "rerank candidate's source bundle is inactive",
            ));
        }
        let unchanged = match target {
            RetrievalTarget::Cognition(reference) if reference.kind == CognitiveKind::Memory => {
                let current = self
                    .memory(namespace, &MemoryId(reference.id.clone()))
                    .await?;
                current.subject_id == *subject && current.revision == reference.revision
            }
            RetrievalTarget::Cognition(reference) => {
                let current = self
                    .structured_object(namespace, &CognitiveId(reference.id.clone()))
                    .await?;
                current.subject_id == *subject && current.reference() == *reference
            }
            RetrievalTarget::Media(MediaReference::Representation { reference }) => self
                .representation_head(namespace, subject, &reference.id)
                .await?
                .is_some_and(|head| head.reference() == *reference),
            // Original and region identities are immutable.
            RetrievalTarget::Media(_) => true,
        };
        if !unchanged {
            return Err(Error::new(
                ErrorCode::Conflict,
                "rerank candidate's current revision changed",
            ));
        }
        Ok(())
    }
}
