use super::*;
use crate::{Database, WriteTransaction, write_error};
impl Database {
    pub async fn context_session(&self, id: &ContextSessionId) -> Result<ContextSession> {
        session(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }
    pub async fn context_history_targets(
        &self,
        id: &ContextSessionId,
    ) -> Result<Vec<RetrievalTarget>> {
        let rows:Vec<String>=sqlx::query_scalar("WITH RECURSIVE ancestors(id) AS (SELECT id FROM context_sessions WHERE id=? UNION ALL SELECT s.parent_id FROM context_sessions s JOIN ancestors a ON s.id=a.id WHERE s.parent_id IS NOT NULL) SELECT DISTINCT r.value FROM context_admissions a,json_each(a.body,'$.sources') r WHERE a.session_id IN(SELECT id FROM ancestors)")
            .bind(id.as_str()).fetch_all(&self.pool).await.map_err(storage_error)?;
        rows.iter().map(|body| decode(body)).collect()
    }
}
impl WriteTransaction {
    pub async fn context_session(&mut self, id: &ContextSessionId) -> Result<ContextSession> {
        session(&mut self.tx, id).await
    }
    pub async fn save_context_session(
        &mut self,
        value: &ContextSession,
        expected_revision: i64,
    ) -> Result<()> {
        value.id.validate()?;
        self.check_sequence(value.commit_seq)?;
        self.context()
            .require_subject(&value.namespace_id, &value.subject_id)?;
        self.subject(&value.namespace_id, &value.subject_id).await?;
        context_text(&value.native_session, 512, true)?;
        context_text(&value.history_generation, 512, true)?;
        let head = match session(&mut self.tx, &value.id).await {
            Ok(row) => Some(row),
            Err(error) if error.code == ErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        if head.as_ref().map_or(0, |row| row.revision) != expected_revision
            || value.revision != self.commit_seq()
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "context Session revision changed",
            ));
        }
        if let Some(head) = head
            && (head.namespace_id != value.namespace_id
                || head.subject_id != value.subject_id
                || head.closed_at.is_some()
                || head.history_generation != value.history_generation
                || head.parent_session != value.parent_session
                || head.native_session != value.native_session)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "context Session binding changed or is closed",
                ));
            }
        if let Some(parent) = &value.parent_session {
            let parent = session(&mut self.tx, parent).await?;
            if parent.namespace_id != value.namespace_id || parent.subject_id != value.subject_id {
                return Err(Error::invalid("parent Session belongs to another subject"));
            }
        }
        if let Some(id) = &value.active_work_context {
            let context = work(&mut self.tx, id).await?;
            if context.namespace_id != value.namespace_id
                || context.subject_id != value.subject_id
                || context.state != WorkContextState::Open
            {
                return Err(Error::invalid(
                    "active WorkContext must be open and belong to this subject",
                ));
            }
        }
        let mut body = serde_json::to_value(value)?;
        body.as_object_mut()
            .expect("Session object")
            .remove("residents");
        sqlx::query("INSERT INTO context_sessions(id,namespace_id,subject_id,parent_id,native_session,history_generation,active_work_context,revision,history_invalidated,body,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET active_work_context=excluded.active_work_context,revision=excluded.revision,history_invalidated=excluded.history_invalidated,body=excluded.body,commit_seq=excluded.commit_seq")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.parent_session.as_ref().map(|id|id.as_str())).bind(&value.native_session).bind(&value.history_generation).bind(value.active_work_context.as_ref().map(|id|id.as_str())).bind(value.revision).bind(value.history_invalidated).bind(encode(&body)?).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub async fn context_sessions_for_work(
        &mut self,
        id: &WorkContextId,
    ) -> Result<Vec<ContextSession>> {
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM context_sessions WHERE active_work_context=?")
                .bind(id.as_str())
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
        let mut result = Vec::new();
        for id in ids {
            result.push(session(&mut self.tx, &ContextSessionId(id)).await?);
        }
        Ok(result)
    }
}
