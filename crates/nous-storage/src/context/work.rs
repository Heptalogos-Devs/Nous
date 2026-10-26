use super::*;
use crate::{Database, WriteTransaction, write_error};

impl Database {
    pub async fn work_context_history(
        &self,
        id: &WorkContextId,
        limit: usize,
    ) -> Result<Vec<WorkContext>> {
        if !(1..=256).contains(&limit) {
            return Err(Error::invalid("WorkContext history limit is out of range"));
        }
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT body FROM work_context_revisions WHERE id=? ORDER BY revision DESC LIMIT ?",
        )
        .bind(id.as_str())
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.iter().map(|body| decode(body)).collect()
    }
    pub async fn work_context(&self, id: &WorkContextId) -> Result<WorkContext> {
        work(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }
    pub async fn list_work_contexts(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        limit: usize,
    ) -> Result<Vec<WorkContext>> {
        if !(1..=256).contains(&limit) {
            return Err(Error::invalid("WorkContext page size is out of range"));
        }
        let mut view = self.read_view().await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT id FROM work_context_revisions w WHERE namespace_id=? AND subject_id=? AND revision=(SELECT max(revision) FROM work_context_revisions n WHERE n.id=w.id) ORDER BY id")
            .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *view.tx).await.map_err(storage_error)?;
        let mut output = Vec::new();
        for id in rows {
            let id = WorkContextId(id);
            match view
                .require_work_context_read(ctx, namespace, subject, &id)
                .await
            {
                Ok(()) => {}
                Err(error) if error.code == ErrorCode::Forbidden => continue,
                Err(error) => return Err(error),
            }
            output.push(work(&mut view.tx, &id).await?);
            if output.len() == limit {
                break;
            }
        }
        view.finish().await?;
        Ok(output)
    }
}
impl WriteTransaction {
    pub async fn work_context(&mut self, id: &WorkContextId) -> Result<WorkContext> {
        work(&mut self.tx, id).await
    }
    pub async fn save_work_context(
        &mut self,
        value: &WorkContext,
        expected_revision: i64,
    ) -> Result<()> {
        value.content.validate()?;
        value.id.validate()?;
        self.check_sequence(value.commit_seq)?;
        self.context()
            .require_subject(&value.namespace_id, &value.subject_id)?;
        self.subject(&value.namespace_id, &value.subject_id).await?;
        let head = match work(&mut self.tx, &value.id).await {
            Ok(row) => Some(row),
            Err(error) if error.code == ErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        if head.as_ref().map_or(0, |row| row.revision) != expected_revision
            || value.revision != self.commit_seq()
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "WorkContext revision changed",
            ));
        }
        if let Some(head) = &head
            && (head.namespace_id != value.namespace_id
                || head.subject_id != value.subject_id
                || head.state == WorkContextState::Ended)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "WorkContext is outside scope or ended",
                ));
            }
        if (value.state == WorkContextState::Ended) != value.ended_at.is_some() {
            return Err(Error::invalid(
                "WorkContext end state and timestamp disagree",
            ));
        }
        sqlx::query("INSERT INTO work_context_revisions(id,revision,namespace_id,subject_id,body,commit_seq) VALUES(?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.revision).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(encode(value)?).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
