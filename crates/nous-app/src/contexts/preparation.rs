use super::*;
impl App {
    pub async fn cancel_context_preparation(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &ContextPreparationId,
    ) -> Result<OperationReceipt> {
        let previous = self.context_preparation(ctx, id).await?;
        let (ns, subject) = previous.job.subject_scope()?;
        let key = operation_key(
            ctx,
            "context.cancel",
            &scope(ns, subject),
            operation_id,
            &json!({"preparation_id":id}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut current = tx.context_preparation(id).await?;
        self.tasks
            .cancel_in(&mut tx, &mut current.job, None)
            .await?;
        if let Some(child) = &current.steward_job_id {
            let mut child = tx.job_record(child).await?;
            self.tasks.cancel_in(&mut tx, &mut child, None).await?;
        }
        tx.commit(json!({"preparation_id":id,"state":current.job.state}))
            .await
    }
    pub async fn prepare_context(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: ContextPrepareRequest,
    ) -> Result<OperationReceipt> {
        request.validate()?;
        let context = if let Some(scope) = &request.source_read {
            ctx.with_source_read(scope.clone())?
        } else if ctx.source_read.is_empty() {
            ctx.with_source_read(SourceReadScope::empty(request.subject_id.clone()))?
        } else {
            ctx.clone()
        };
        let ctx = &context;
        let ns = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&ns, &request.subject_id)?;
        let key = operation_key(
            ctx,
            "context.prepare",
            &scope(&ns, &request.subject_id),
            operation_id,
            &request,
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let captured = self.capture_context(ctx, &ns, &request).await?;
        let resident_limit = self
            .context_resident_limit(&ns, &request.subject_id)
            .await?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let job = self.tasks.new_job(
            &tx,
            "context.prepare",
            Some(ns),
            Some(request.subject_id.clone()),
        );
        self.tasks.submit_in(&mut tx, &job).await?;
        let mut preparation = ContextPreparation {
            id: ContextPreparationId::new(),
            job,
            request,
            config_digest: Some(captured.config_digest),
            policy: Some(captured.policy),
            catalog: captured.catalog,
            work_context: captured.work_context,
            session_revision: Some(captured.session_revision),
            source_watermark: Some(captured.watermark),
            steward_job_id: None,
            response: None,
            degradation: captured.degradation,
        };
        for segment in &preparation.catalog {
            tx.validate_context_targets(
                preparation.job.namespace_id.as_ref().expect("namespace"),
                &preparation.request.subject_id,
                &segment.sources,
            )
            .await?;
        }
        if !preparation.request.current_sources.is_empty() {
            let admissions = preparation
                .request
                .current_sources
                .iter()
                .map(|target| ContextResidentAdmission {
                    target: target.clone(),
                    state: ContextResidentState::Provisional,
                    reason: "current_input".into(),
                    hold_until: None,
                    last_meaningful_use_at: None,
                })
                .collect::<Vec<_>>();
            let outcome = tx
                .apply_context_residents(
                    &preparation.request.session_id,
                    captured.session_revision,
                    &admissions,
                    resident_limit,
                )
                .await?;
            preparation.session_revision = Some(outcome.revision);
        }
        tx.insert_context_preparation(&preparation).await?;
        tx.commit(json!({"preparation_id":preparation.id,"job_id":preparation.job.id}))
            .await
    }
}
