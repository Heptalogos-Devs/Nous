//! Source erasure delegates captured payloads and running work to their owners.
use crate::App;
use nous_domain::*;
use nous_storage::WriteTransaction;
impl App {
    pub(crate) async fn purge_source_dependents_in(
        &self,
        tx: &mut WriteTransaction,
        namespace: &NamespaceId,
        subject: &SubjectId,
        targets: &[RetrievalTarget],
    ) -> Result<()> {
        let mut jobs = tx
            .purge_context_sources(namespace, subject, targets)
            .await?;
        jobs.extend(
            tx.retire_model_source_payloads(namespace, subject, targets)
                .await?,
        );
        jobs.sort();
        jobs.dedup();
        for id in jobs {
            let mut job = tx.job_record(&id).await?;
            self.tasks
                .cancel_in(
                    tx,
                    &mut job,
                    Some(Error::new(
                        ErrorCode::Cancelled,
                        "task source was permanently purged",
                    )),
                )
                .await?;
        }
        Ok(())
    }
}
