use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Accepted {
    request: ProcessImportUnitsRequest,
}
pub(super) struct UnitJob {
    pub unit: ImportTextUnit,
    pub job: Option<JobRecord>,
    pub output: Option<ImportUnitOutput>,
    pub vector_error: Option<Error>,
}
pub(super) struct Task {
    pub job: JobRecord,
    pub request: ProcessImportUnitsRequest,
    pub catalog: ImportUnitCatalog,
    pub inputs: Vec<ImportInputSource>,
    pub units: Vec<UnitJob>,
}
pub(super) async fn read(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    id: &str,
    cancel: &CancellationToken,
) -> Result<Task> {
    let value = app.job(ctx, id).await?;
    let job: JobRecord = serde_json::from_value(value["job"].clone())?;
    let (namespace, subject) = job.subject_scope()?;
    if job.kind != "import.process"
        || namespace != &binding.namespace_id
        || subject != &binding.subject_id
    {
        return Err(Error::new(
            ErrorCode::NotFound,
            "import processing task was not found in this subject",
        ));
    }
    let accepted: Accepted = serde_json::from_value(
        job.checkpoint
            .clone()
            .ok_or_else(|| Error::internal("import task has no accepted request"))?,
    )?;
    let request = accepted.request;
    let view = app
        .retry_import_inputs(
            ctx,
            RetryImportInputsRequest {
                namespace_id: binding.namespace_id.clone(),
                subject_id: binding.subject_id.clone(),
                catalog_id: request.catalog_id.clone(),
                unit_ids: request
                    .unit_ids
                    .clone()
                    .ok_or_else(|| Error::internal("import task has no frozen UnitId selection"))?,
            },
            cancel,
        )
        .await?;
    let references: Option<ImportProcessingResult> =
        job.result.clone().map(serde_json::from_value).transpose()?;
    let mut units = Vec::new();
    for unit in view.units {
        let child: Option<JobRecord> = if let Some(reference) = references
            .as_ref()
            .and_then(|result| result.unit_jobs.iter().find(|item| item.unit_id == unit.id))
        {
            let value = app.job(ctx, &reference.job_id).await?;
            Some(serde_json::from_value(value["job"].clone())?)
        } else {
            None
        };
        let processing: Option<ImportUnitProcessResult> = child
            .as_ref()
            .and_then(|job| job.result.clone())
            .map(serde_json::from_value)
            .transpose()?;
        let output = if let Some(result) = &processing {
            Some(
                app.import_unit_output(
                    ctx,
                    &binding.namespace_id,
                    &binding.subject_id,
                    &result.output_id,
                )
                .await?,
            )
        } else {
            None
        };
        units.push(UnitJob {
            unit,
            job: child,
            output,
            vector_error: processing.and_then(|result| result.vector_error),
        });
    }
    Ok(Task {
        job,
        request,
        catalog: view.catalog,
        inputs: view.input_sources,
        units,
    })
}
pub(super) fn status(job: Option<&JobRecord>) -> &'static str {
    match job.map(|job| job.state) {
        None | Some(JobState::Queued) => "queued",
        Some(JobState::Waiting) => "waiting",
        Some(JobState::Running) => "running",
        Some(JobState::Succeeded) => "completed",
        Some(JobState::Failed) => "failed",
        Some(JobState::Cancelled) => "cancelled",
    }
}
pub(super) fn step(job: Option<&JobRecord>) -> String {
    match job {
        Some(job) if job.state == JobState::Waiting => format!(
            "waiting_{}",
            job.wait
                .as_ref()
                .map(|wait| wait.kind.as_str())
                .unwrap_or("unknown")
        ),
        Some(job) if job.result.is_some() && job.state == JobState::Running => "indexing".into(),
        _ => status(job).into(),
    }
}
pub(super) fn stamp(value: &str) -> Result<f64> {
    Ok(chrono::DateTime::parse_from_rfc3339(value)
        .map_err(Error::dependency)?
        .timestamp_millis() as f64
        / 1000.0)
}
