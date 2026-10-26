use super::*;
impl App {
    pub async fn work_context_history(
        &self,
        ctx: &CallContext,
        id: &WorkContextId,
        limit: usize,
    ) -> Result<Vec<WorkContext>> {
        self.get_work_context(ctx, id).await?;
        self.database.work_context_history(id, limit).await
    }
    pub async fn create_work_context(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: CreateWorkContextRequest,
    ) -> Result<OperationReceipt> {
        request.content.validate()?;
        let ns = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&ns, &request.subject_id)?;
        for target in &request.content.anchors {
            self.context_segment(ctx, &ns, &request.subject_id, target, true)
                .await?;
        }
        let key = operation_key(
            ctx,
            "work_context.create",
            &scope(&ns, &request.subject_id),
            operation_id,
            &request,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        tx.validate_context_targets(&ns, &request.subject_id, &request.content.anchors)
            .await?;
        let value = WorkContext {
            id: WorkContextId::new(),
            namespace_id: ns,
            subject_id: request.subject_id,
            revision: tx.commit_seq(),
            state: WorkContextState::Open,
            content: request.content,
            created_at: tx.time().recorded(),
            recorded_at: tx.time().recorded(),
            ended_at: None,
            commit_seq: tx.commit_seq(),
        };
        tx.save_work_context(&value, 0).await?;
        tx.commit(json!(value)).await
    }
    pub async fn get_work_context(
        &self,
        ctx: &CallContext,
        id: &WorkContextId,
    ) -> Result<WorkContext> {
        let value = self.database.work_context(id).await?;
        ctx.require_subject(&value.namespace_id, &value.subject_id)?;
        let mut read = self.database.read_view().await?;
        read.require_work_context_read(ctx, &value.namespace_id, &value.subject_id, id)
            .await?;
        read.finish().await?;
        Ok(value)
    }
    pub async fn list_work_contexts(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        limit: usize,
    ) -> Result<Vec<WorkContext>> {
        let ns = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&ns, subject)?;
        self.database
            .list_work_contexts(ctx, &ns, subject, limit)
            .await
    }
    pub async fn change_work_context(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &WorkContextId,
        expected_revision: i64,
        change: WorkContextChange,
    ) -> Result<OperationReceipt> {
        let previous = self.get_work_context(ctx, id).await?;
        let key = operation_key(
            ctx,
            "work_context.change",
            &scope(&previous.namespace_id, &previous.subject_id),
            operation_id,
            &json!({"id":id,"expected_revision":expected_revision,"change":change}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut value = tx.work_context(id).await?;
        if value.revision != expected_revision || value.state == WorkContextState::Ended {
            return Err(Error::new(
                ErrorCode::Conflict,
                "WorkContext is stale or ended",
            ));
        }
        match change {
            WorkContextChange::Edit { content } => {
                content.validate()?;
                value.content = content;
            }
            WorkContextChange::Pin { anchors } => {
                for target in anchors {
                    if !value.content.anchors.contains(&target) {
                        value.content.anchors.push(target);
                    }
                }
            }
            WorkContextChange::Unpin { anchors } => value
                .content
                .anchors
                .retain(|target| !anchors.contains(target)),
            WorkContextChange::ClearText => value.content.context_text.clear(),
            WorkContextChange::ClearAnchors => value.content.anchors.clear(),
            WorkContextChange::Pause if value.state == WorkContextState::Open => {
                value.state = WorkContextState::Paused
            }
            WorkContextChange::Resume if value.state == WorkContextState::Paused => {
                value.state = WorkContextState::Open
            }
            WorkContextChange::End => {
                value.state = WorkContextState::Ended;
                value.ended_at = Some(tx.time().recorded());
            }
            _ => {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "WorkContext transition is not applicable",
                ));
            }
        }
        value.content.validate()?;
        tx.validate_context_targets(
            &value.namespace_id,
            &value.subject_id,
            &value.content.anchors,
        )
        .await?;
        value.revision = tx.commit_seq();
        value.commit_seq = tx.commit_seq();
        value.recorded_at = tx.time().recorded();
        tx.save_work_context(&value, expected_revision).await?;
        if value.state != WorkContextState::Open {
            for mut session in tx.context_sessions_for_work(id).await? {
                let expected = session.revision;
                session.active_work_context = None;
                session.revision = tx.commit_seq();
                session.commit_seq = tx.commit_seq();
                session.recorded_at = tx.time().recorded();
                tx.save_context_session(&session, expected).await?;
            }
        }
        tx.commit(json!(value)).await
    }
}
