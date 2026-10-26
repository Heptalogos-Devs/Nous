use super::*;
impl App {
    pub async fn open_context_session(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: OpenContextSessionRequest,
    ) -> Result<OperationReceipt> {
        let ns = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&ns, &request.subject_id)?;
        context_text(&request.native_session, 512, true)?;
        context_text(&request.history_generation, 512, true)?;
        let key = operation_key(
            ctx,
            "context.session_open",
            &scope(&ns, &request.subject_id),
            operation_id,
            &request,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut invalidated = false;
        if let Some(id) = &request.parent_session {
            let parent = tx.context_session(id).await?;
            if parent.namespace_id != ns || parent.subject_id != request.subject_id {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "parent Session belongs to another subject",
                ));
            }
            invalidated = parent.history_invalidated;
        }
        let value = ContextSession {
            id: ContextSessionId::new(),
            namespace_id: ns,
            subject_id: request.subject_id,
            native_session: request.native_session,
            parent_session: request.parent_session,
            history_generation: request.history_generation,
            active_work_context: None,
            revision: tx.commit_seq(),
            history_invalidated: invalidated,
            residents: Vec::new(),
            created_at: tx.time().recorded(),
            recorded_at: tx.time().recorded(),
            closed_at: None,
            commit_seq: tx.commit_seq(),
        };
        tx.save_context_session(&value, 0).await?;
        tx.commit(json!(value)).await
    }
    pub async fn get_context_session(
        &self,
        ctx: &CallContext,
        id: &ContextSessionId,
    ) -> Result<ContextSession> {
        let value = self.database.context_session(id).await?;
        ctx.require_subject(&value.namespace_id, &value.subject_id)?;
        Ok(value)
    }
    pub async fn change_context_session(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &ContextSessionId,
        expected_revision: i64,
        change: ContextSessionChange,
    ) -> Result<OperationReceipt> {
        let previous = self.get_context_session(ctx, id).await?;
        let key = operation_key(
            ctx,
            "context.session_change",
            &scope(&previous.namespace_id, &previous.subject_id),
            operation_id,
            &json!({"id":id,"expected_revision":expected_revision,"change":change}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut value = tx.context_session(id).await?;
        if value.revision != expected_revision {
            return Err(Error::new(
                ErrorCode::Conflict,
                "context Session revision changed",
            ));
        }
        value.active_work_context = change.active_work_context;
        if change.close {
            value.closed_at = Some(tx.time().recorded());
            value.active_work_context = None;
        }
        value.revision = tx.commit_seq();
        value.commit_seq = tx.commit_seq();
        value.recorded_at = tx.time().recorded();
        tx.save_context_session(&value, expected_revision).await?;
        tx.commit(json!(value)).await
    }
}
