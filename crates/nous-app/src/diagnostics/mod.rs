mod accounting;
mod runtime;
use crate::App;
use nous_domain::*;
use std::collections::BTreeMap;

impl App {
    pub async fn subject_diagnostics(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
    ) -> Result<SubjectDiagnostics> {
        let mut view = self.database.read_view().await?;
        let namespace = view.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        let inventory = view.subject_inventory(ctx, &namespace, subject).await?;
        let vector_spaces = view
            .subject_vector_diagnostics(ctx, &namespace, subject)
            .await?;
        let mut work = WorkDiagnostics::default();
        let mut jobs = JobListRequest {
            namespace_id: Some(namespace.clone()),
            subject_id: Some(subject.clone()),
            limit: 200,
            ..Default::default()
        };
        loop {
            let page = view.list_jobs(ctx, &jobs).await?;
            for job in page.items {
                *work
                    .by_kind
                    .entry(job.kind.clone())
                    .or_default()
                    .entry(accounting::name(&job.state)?)
                    .or_default() += 1;
                if let Some(reason) = &job.waiting_for {
                    *work.waiting_by_reason.entry(reason.clone()).or_default() += 1;
                }
                if let Some(queue) = view.queue_record(&job.id).await? {
                    *work.queue_rows_by_status.entry(queue.status).or_default() += 1;
                } else {
                    work.jobs_without_queue_row += 1;
                }
                if job.kind == "amemorix.maintain" && job.state == JobState::Succeeded {
                    latest(&mut work.last_maintenance_at, &job.updated_at)?;
                }
            }
            match page.next_cursor {
                Some(cursor) => jobs.cursor = Some(cursor),
                None => break,
            }
        }
        for receipt in view
            .subject_maintenance_receipts(&namespace, subject)
            .await?
        {
            match ctx.require_accepted_source_read(&receipt.context) {
                Ok(()) => latest(&mut work.last_maintenance_at, &receipt.time.recorded())?,
                Err(error) if hidden(&error) => {}
                Err(error) => return Err(error),
            }
        }
        let mut accounting = accounting::Accounting::new();
        for id in view.subject_model_call_ids(&namespace, subject).await? {
            match self.calls.read_in(&mut view, ctx, &id).await {
                Ok(call) => accounting.record(&call)?,
                Err(error) if hidden(&error) => {}
                Err(error) => return Err(error),
            }
        }
        let scope = ConfigScope::Subject {
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
        };
        let configuration = self
            .configuration
            .snapshot_in(&mut view, &scope, false)
            .await?;
        let watermark = view.watermark;
        view.finish().await?;
        let runtime = self
            .subject_runtime_diagnostics(&configuration, ctx)
            .await?;
        let unknown=BTreeMap::from([
            ("profiles".into(),"No persistent Profile snapshot owner is registered; Entity and profile evidence views are separate facts.".into()),
            ("stale_paragraph_marks".into(),"No paragraph stale-mark queue owner is registered.".into()),
            ("profile_refresh".into(),"No Profile refresh queue owner is registered.".into()),
            ("episode_rebuild".into(),"Generic cognition formation jobs do not constitute an Episode rebuild queue.".into()),
            ("legacy_vector_pools".into(),"Native storage uses immutable semantic spaces rather than single/dual file pools.".into()),
            ("auto_save".into(),"Authority commits durably per transaction; no toggleable file-pool auto-save owner is registered.".into()),
            ("provider_health".into(),"Local readiness does not perform a provider request; actual attempts are in the model ledger.".into()),
        ]);
        Ok(SubjectDiagnostics {
            namespace_id: namespace,
            subject_id: subject.clone(),
            watermark,
            inventory,
            work,
            vector_spaces,
            models: accounting.finish(),
            runtime,
            unknown,
        })
    }
}
fn hidden(error: &Error) -> bool {
    matches!(error.code, ErrorCode::Forbidden | ErrorCode::NotFound)
}
fn latest(current: &mut Option<String>, candidate: &str) -> Result<()> {
    let time = OperationTime::parse(candidate)?;
    if current
        .as_ref()
        .map(|value| OperationTime::parse(value))
        .transpose()?
        .is_none_or(|old| old.utc() < time.utc())
    {
        *current = Some(candidate.into());
    }
    Ok(())
}
