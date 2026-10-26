//! Target withdrawal and content erasure are separate operations under one writer lock.
use crate::source::dependents::memory_dependents_cte;
use crate::{WriteTransaction, storage_error, write_error};
use nous_domain::*;
use sqlx::Row;
use std::collections::BTreeSet;
const MATCHES: &str = "json_extract(target,'$.target.kind')='cognition' AND EXISTS(SELECT 1 FROM affected a WHERE a.kind=json_extract(target,'$.target.reference.kind') AND a.id=json_extract(target,'$.target.reference.id') AND a.revision=json_extract(target,'$.target.reference.revision'))";
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ContentKey {
    namespace: NamespaceId,
    subject: SubjectId,
    space: String,
    hash: String,
}
impl WriteTransaction {
    pub(crate) async fn retire_cognition_embedding_jobs(
        &mut self,
        value: &StructuredObject,
    ) -> Result<()> {
        let identity = RetrievalTarget::Cognition(value.reference()).identity_key()?;
        let rows=sqlx::query("SELECT DISTINCT namespace_id,subject_id,space_id,source_hash FROM embedding_subscriptions WHERE namespace_id=? AND subject_id=? AND identity_key=?")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&identity).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let keys = rows.into_iter().map(content_key).collect::<Vec<_>>();
        sqlx::query("DELETE FROM embedding_subscriptions WHERE namespace_id=? AND subject_id=? AND identity_key=?")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(identity).execute(&mut *self.tx).await.map_err(write_error)?;
        for key in keys {
            self.reconcile_embedding_content(&key, false).await?;
        }
        Ok(())
    }
    pub(crate) async fn purge_memory_embeddings(&mut self, id: &MemoryId) -> Result<()> {
        let mut keys = BTreeSet::new();
        for table in ["embedding_subscriptions", "embedding_records"] {
            let mut query = memory_dependents_cte(id);
            query
                .push(" SELECT DISTINCT namespace_id,subject_id,space_id,source_hash FROM ")
                .push(table)
                .push(" WHERE ")
                .push(MATCHES);
            keys.extend(
                query
                    .build()
                    .fetch_all(&mut *self.tx)
                    .await
                    .map_err(storage_error)?
                    .into_iter()
                    .map(content_key),
            );
            let mut delete = memory_dependents_cte(id);
            delete
                .push(" DELETE FROM ")
                .push(table)
                .push(" WHERE ")
                .push(MATCHES);
            delete
                .build()
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
        }
        for key in keys {
            self.reconcile_embedding_content(&key, true).await?;
        }
        Ok(())
    }
    async fn reconcile_embedding_content(
        &mut self,
        key: &ContentKey,
        permanent: bool,
    ) -> Result<()> {
        if self.has_embedding_consumers(key).await? {
            return Ok(());
        }
        let jobs:Vec<String>=sqlx::query_scalar("SELECT r.job_id FROM embedding_runs r JOIN task_records j ON j.job_id=r.job_id WHERE j.namespace_id=? AND j.subject_id=? AND r.space_id=? AND EXISTS(SELECT 1 FROM json_each(r.content_hashes) WHERE value=?)")
            .bind(key.namespace.as_str()).bind(key.subject.as_str()).bind(&key.space).bind(&key.hash).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        sqlx::query("DELETE FROM embedding_content_claims WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=?")
            .bind(key.namespace.as_str()).bind(key.subject.as_str()).bind(&key.space).bind(&key.hash).execute(&mut *self.tx).await.map_err(write_error)?;
        if permanent {
            sqlx::query("DELETE FROM embedding_contents WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=?")
                .bind(key.namespace.as_str()).bind(key.subject.as_str()).bind(&key.space).bind(&key.hash).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        for job in jobs {
            if permanent {
                for call in self.model_step_calls(&job, "embedding").await? {
                    self.redact_embedding_hash(&call, &key.hash).await?;
                }
            }
            let error = Error::new(
                ErrorCode::Cancelled,
                "embedding work has no remaining consumers",
            );
            sqlx::query("UPDATE task_records SET state='cancelled',wait=NULL,error=?,updated_at=? WHERE job_id=? AND state IN('queued','waiting','running') AND NOT EXISTS(SELECT 1 FROM embedding_content_claims WHERE job_id=task_records.job_id) AND NOT EXISTS(SELECT 1 FROM model_calls WHERE job_id=task_records.job_id)")
                .bind(serde_json::to_string(&error)?).bind(self.time().recorded()).bind(job).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(())
    }
    async fn has_embedding_consumers(&mut self, key: &ContentKey) -> Result<bool> {
        let mut live = false;
        for subscription in self
            .embedding_subscriptions(&key.namespace, &key.subject, &key.space, &key.hash)
            .await?
        {
            match self
                .validate_embedding_input(&key.namespace, &key.subject, &subscription.input)
                .await
            {
                Ok(()) => live = true,
                Err(error) if unavailable(&error) => {
                    self.unsubscribe_embedding(&subscription).await?
                }
                Err(error) => return Err(error),
            }
        }
        let records=sqlx::query("SELECT target,source_key FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=?")
            .bind(key.namespace.as_str()).bind(key.subject.as_str()).bind(&key.space).bind(&key.hash).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in records {
            let target: EmbeddingTarget = serde_json::from_str(row.get("target"))?;
            let valid = match target {
                EmbeddingTarget::Item(RetrievalTarget::Cognition(reference)) => {
                    super::source::validate_reference(
                        &mut self.tx,
                        &key.namespace,
                        &key.subject,
                        &reference,
                    )
                    .await
                }
                EmbeddingTarget::Item(RetrievalTarget::Media(reference)) => {
                    super::media_source::authorize(
                        &mut self.tx,
                        &key.namespace,
                        &key.subject,
                        &reference,
                    )
                    .await
                    .map(|_| ())
                }
                EmbeddingTarget::Tag | EmbeddingTarget::Query => Ok(()),
            };
            match valid {
                Ok(()) => live = true,
                Err(error) if unavailable(&error) => {
                    sqlx::query("DELETE FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=? AND source_key=?")
                        .bind(key.namespace.as_str()).bind(key.subject.as_str()).bind(&key.space).bind(&key.hash).bind(row.get::<String,_>("source_key")).execute(&mut *self.tx).await.map_err(write_error)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(live)
    }
}
fn content_key(row: sqlx::sqlite::SqliteRow) -> ContentKey {
    ContentKey {
        namespace: NamespaceId(row.get("namespace_id")),
        subject: SubjectId(row.get("subject_id")),
        space: row.get("space_id"),
        hash: row.get("source_hash"),
    }
}
fn unavailable(error: &Error) -> bool {
    matches!(
        error.code,
        ErrorCode::NotFound | ErrorCode::Conflict | ErrorCode::Forbidden
    )
}
