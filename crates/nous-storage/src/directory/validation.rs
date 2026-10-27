use crate::{QuerySnapshotOptions, ReadView};
use nous_domain::*;
impl ReadView {
    /// Binding an address never grants source visibility or bypasses the query view.
    pub async fn require_retrieval_reference(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        options: &QuerySnapshotOptions<'_>,
        target: &RetrievalTarget,
    ) -> Result<()> {
        match target {
            RetrievalTarget::Cognition(reference) => {
                self.cognitive_reference_snapshot(namespace, subject, options, reference)
                    .await?;
            }
            RetrievalTarget::Media(reference) => {
                self.media_reference_snapshot(namespace, subject, options, reference)
                    .await?;
            }
        }
        Ok(())
    }
}
