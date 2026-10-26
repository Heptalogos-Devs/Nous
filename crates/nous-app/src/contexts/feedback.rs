use super::*;
use std::collections::BTreeSet;

impl App {
    pub async fn admit_context(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: ContextAdmitRequest,
    ) -> Result<OperationReceipt> {
        request.measurement.validate()?;
        context_text(&request.invocation_id, 512, true)?;
        context_text(&request.history_generation, 512, true)?;
        if request.rendered_digest.len() != 64
            || !request
                .rendered_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::invalid(
                "host-rendered input digest must be lowercase SHA-256",
            ));
        }
        let preparation = self
            .context_preparation(ctx, &request.preparation_id)
            .await?;
        let response = preparation.response.as_ref().ok_or_else(|| {
            Error::new(ErrorCode::Conflict, "context preparation is not complete")
        })?;
        let projection = self
            .database
            .context_projection(&response.cursor.projection_id)
            .await?;
        let (ns, subject) = (&projection.namespace_id, &projection.subject_id);
        let context = accepted_context(ctx, &projection.source_read)?;
        let ctx = &context;
        ctx.require_subject(ns, subject)?;
        let key = operation_key(
            ctx,
            "context.admit",
            &scope(ns, subject),
            operation_id,
            &request,
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let (_, policy_digest) = self
            .context_policy(ns, subject, &projection.consumer_id)
            .await?;
        let current_digest = super::compile::hash(
            &json!({"policy":policy_digest,"source_read":ctx.source_read_identity()?}),
        )?;
        if current_digest != projection.config_digest {
            return Err(Error::new(
                ErrorCode::Conflict,
                "context consumer policy changed before model admission",
            )
            .with_context("context_reset_required", true));
        }
        let selected = request
            .applied_segment_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if selected.len() != request.applied_segment_ids.len()
            || selected.iter().any(|key| {
                !projection
                    .segments
                    .iter()
                    .any(|segment| &segment.key == key)
            })
            || projection
                .segments
                .iter()
                .any(|segment| segment.required && !selected.contains(&segment.key))
        {
            return Err(Error::invalid(
                "admitted input omits a required unit or names a foreign segment",
            ));
        }
        let segments = projection
            .segments
            .iter()
            .filter(|segment| selected.contains(&segment.key))
            .collect::<Vec<_>>();
        let sources = segments
            .iter()
            .flat_map(|segment| segment.sources.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let dependencies = segments
            .iter()
            .flat_map(|segment| segment.dependencies.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for target in &sources {
            self.context_segment(ctx, ns, subject, target, false)
                .await?;
        }
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let session = tx.context_session(&projection.session_id).await?;
        if session.revision != projection.session_revision {
            return Err(Error::new(
                ErrorCode::Conflict,
                "context Session changed before model admission",
            ));
        }
        if let Some(context) = &projection.work_context {
            let head = tx.work_context(&context.id).await?;
            if head.revision != context.revision || head.state != WorkContextState::Open {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "WorkContext changed before model admission",
                ));
            }
        }
        let value = ContextAdmission {
            id: ContextAdmissionId::new(),
            projection_id: projection.id.clone(),
            namespace_id: ns.clone(),
            subject_id: subject.clone(),
            session_id: projection.session_id,
            consumer_id: projection.consumer_id,
            instance_id: projection.instance_id,
            invocation_id: request.invocation_id,
            rendered_digest: request.rendered_digest,
            history_generation: request.history_generation,
            segments: request.applied_segment_ids,
            sources,
            dependencies,
            measurement: request.measurement,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        tx.save_context_admission(&value).await?;
        tx.commit(json!(value)).await
    }
    pub async fn report_context(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: ContextReportRequest,
    ) -> Result<OperationReceipt> {
        request.validate()?;
        let admitted = self.context_admission(ctx, &request.admission_id).await?;
        let key = operation_key(
            ctx,
            "context.report",
            &scope(&admitted.namespace_id, &admitted.subject_id),
            operation_id,
            &request,
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let resident_limit = self
            .context_resident_limit(&admitted.namespace_id, &admitted.subject_id)
            .await?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let (_, added) = tx.record_context_report(&request).await?;
        if added > 0 {
            let session = tx.context_session(&admitted.session_id).await?;
            if session.closed_at.is_none() && !session.history_invalidated {
                let admissions = request
                    .uses
                    .iter()
                    .map(|event| ContextResidentAdmission {
                        target: event.target.clone(),
                        state: ContextResidentState::Resident,
                        reason: "meaningful_use".into(),
                        hold_until: None,
                        last_meaningful_use_at: Some(request.occurred_at.clone()),
                    })
                    .collect::<Vec<_>>();
                tx.apply_context_residents(
                    &session.id,
                    session.revision,
                    &admissions,
                    resident_limit,
                )
                .await?;
            }
        }
        tx.commit(
            json!({"admission_id":admitted.id,"outcome":request.outcome,"accepted_uses":added}),
        )
        .await
    }
}
