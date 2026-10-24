use crate::{WriteTransaction, storage_error, write_error};
use nous_domain::*;
impl WriteTransaction {
    pub(crate) async fn retire_cognition_embedding_jobs(
        &mut self,
        value: &StructuredObject,
    ) -> Result<()> {
        let error = Error::new(ErrorCode::Conflict, "embedding source was retired");
        sqlx::query("UPDATE task_records SET state='cancelled',wait=NULL,error=?,updated_at=? WHERE namespace_id=? AND subject_id=? AND state IN('queued','waiting','running') AND job_id IN(SELECT job_id FROM embedding_inputs WHERE cognition_id=?)")
   .bind(serde_json::to_string(&error)?).bind(self.time().recorded()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.id.as_str()).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub(crate) async fn purge_memory_embeddings(&mut self, id: &MemoryId) -> Result<()> {
        let jobs:Vec<String>=sqlx::query_scalar("WITH RECURSIVE affected(kind,id,revision) AS(SELECT 'memory',memory_id,revision FROM memory_revisions WHERE memory_id=? UNION SELECT r.kind,l.object_id,l.revision FROM structured_links l JOIN structured_revisions r ON r.object_id=l.object_id AND r.revision=l.revision JOIN affected a ON a.kind=l.source_kind AND a.id=l.source_id AND a.revision=l.source_revision) SELECT DISTINCT i.job_id FROM embedding_inputs i WHERE EXISTS(SELECT 1 FROM affected a WHERE a.kind=i.cognition_kind AND a.id=i.cognition_id AND a.revision=i.cognition_revision)").bind(id.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for job in jobs {
            self.redact_job_calls(&job, "memory_purged").await?;
            let error = Error::new(
                ErrorCode::NotFound,
                "embedding source was permanently purged",
            );
            sqlx::query("UPDATE task_records SET state=CASE WHEN state IN('queued','waiting','running') THEN 'cancelled' ELSE state END,wait=NULL,error=?,updated_at=? WHERE job_id=?").bind(serde_json::to_string(&error)?).bind(self.time().recorded()).bind(&job).execute(&mut *self.tx).await.map_err(write_error)?;
            sqlx::query(
                "UPDATE embedding_inputs SET text='' WHERE job_id=? AND target_kind='cognition'",
            )
            .bind(&job)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        }
        sqlx::query("DELETE FROM embedding_records WHERE(cognition_kind='memory' AND cognition_id=?) OR(cognition_kind='memory_semantics' AND EXISTS(SELECT 1 FROM structured_links l WHERE l.object_id=embedding_records.cognition_id AND l.revision=embedding_records.cognition_revision AND l.source_kind='memory' AND l.source_id=?))").bind(id.as_str()).bind(id.as_str()).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
