//! Source erasure delegates context payloads and running work to their owners.
use crate::App;
use nous_domain::*;
use nous_storage::WriteTransaction;
impl App {
    pub(crate) async fn purge_context_dependencies_in(
        &self,
        tx: &mut WriteTransaction,
        namespace: &NamespaceId,
        subject: &SubjectId,
        targets: &[RetrievalTarget],
    ) -> Result<()> {
        for id in tx
            .purge_context_sources(namespace, subject, targets)
            .await?
        {
            let mut job = tx.job_record(&id).await?;
            self.tasks
                .cancel_in(
                    tx,
                    &mut job,
                    Some(Error::new(
                        ErrorCode::Cancelled,
                        "context source was permanently purged",
                    )),
                )
                .await?;
        }
        Ok(())
    }
}
