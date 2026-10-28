use super::{
    task::{self, Task, UnitJob},
    *,
};

fn counts(units: &[&UnitJob]) -> (usize, usize, usize) {
    (
        units
            .iter()
            .filter(|unit| {
                unit.job
                    .as_ref()
                    .is_some_and(|job| job.state == JobState::Succeeded)
            })
            .count(),
        units
            .iter()
            .filter(|unit| {
                unit.job
                    .as_ref()
                    .is_some_and(|job| job.state == JobState::Failed)
            })
            .count(),
        units
            .iter()
            .filter(|unit| {
                unit.job
                    .as_ref()
                    .is_some_and(|job| job.state == JobState::Cancelled)
            })
            .count(),
    )
}
fn aggregate(units: &[&UnitJob]) -> &'static str {
    let (done, failed, cancelled) = counts(units);
    if cancelled > 0 && done + failed + cancelled == units.len() {
        "cancelled"
    } else if failed > 0 && done + failed + cancelled == units.len() {
        if done > 0 {
            "completed_with_errors"
        } else {
            "failed"
        }
    } else if done == units.len() {
        "completed"
    } else if units.iter().any(|unit| {
        unit.job
            .as_ref()
            .is_some_and(|job| job.state != JobState::Queued)
    }) {
        "running"
    } else {
        "queued"
    }
}
pub(super) fn chunk(unit: &UnitJob) -> Result<Value> {
    let job = unit.job.as_ref();
    Ok(
        json!({"chunk_id":unit.unit.id,"index":unit.unit.ordinal,"chunk_type":unit.unit.effective_strategy,
        "status":task::status(job),"step":task::step(job),"failed_at":if job.is_some_and(|job|job.state==JobState::Failed){Value::Null}else{json!("")},
        "retryable":job.is_some_and(|job|job.state==JobState::Failed),"error":job.and_then(|job|job.error.as_ref()).map(|error|error.message.clone()).unwrap_or_default(),
        "progress":if job.is_some_and(|job|matches!(job.state,JobState::Succeeded|JobState::Failed|JobState::Cancelled)){1.0}else{0.0},
        "content_preview":unit.unit.text.chars().take(120).collect::<String>(),
        "updated_at":task::stamp(job.map(|job|job.updated_at.as_str()).unwrap_or(&unit.unit.recorded_at))?,
        "native_job_id":job.map(|job|job.id.as_str()),"native_state":job.map(|job|job.state),"output":unit.output,"vector_error":unit.vector_error,
        "unknown":{"failed_at":"Task errors do not assign a legacy extracting/writing stage"}}),
    )
}
pub(super) fn file(task: &Task, input: &ImportInputSource, include_chunks: bool) -> Result<Value> {
    let units = task
        .units
        .iter()
        .filter(|unit| unit.unit.input_source_id == input.id)
        .collect::<Vec<_>>();
    let (done, failed, cancelled) = counts(&units);
    let strategy = task
        .catalog
        .documents
        .iter()
        .find(|document| document.input_source_id == input.id)
        .map(|document| document.selected_strategy);
    let warnings = units
        .iter()
        .flat_map(|unit| {
            unit.output
                .as_ref()
                .and_then(|output| output.temporal_error.as_ref())
                .into_iter()
                .chain(unit.vector_error.as_ref())
        })
        .map(|error| error.message.clone())
        .collect::<Vec<_>>();
    let mut value = json!({"file_id":input.id,"name":input.name.clone().unwrap_or_else(||"文本输入".into()),"source_kind":"native_input","input_mode":input.format,
        "status":aggregate(&units),"current_step":aggregate(&units),"detected_strategy_type":strategy,
        "total_chunks":units.len(),"done_chunks":done,"failed_chunks":failed,"cancelled_chunks":cancelled,
        "progress":if units.is_empty(){0.0}else{done as f64/units.len() as f64},"error":units.iter().find_map(|unit|unit.job.as_ref().and_then(|job|job.error.as_ref())).map(|error|error.message.clone()).unwrap_or_default(),
        "created_at":task::stamp(&input.recorded_at)?,"updated_at":task::stamp(&task.job.updated_at)?,
        "source_path":null,"content_hash":input.content_hash,"imported_sources":null,"retry_chunk_indexes":[],"retry_mode":if task.request.retry_parent_job_id.is_some(){"unit"}else{""},
        "warning_count":if units.iter().all(|unit|unit.output.is_some()){Some(warnings.len())}else{None},"warnings":warnings,"native":input,
        "unknown":{"imported_sources":"Native output retains typed source regions rather than legacy source strings","source_path":"Paths remain in the caller file boundary"}});
    if include_chunks {
        value["chunks"] = json!(units.into_iter().map(chunk).collect::<Result<Vec<_>>>()?);
    }
    Ok(value)
}
pub(super) fn summary(task: &Task, detail: bool, include_chunks: bool) -> Result<Value> {
    let units = task.units.iter().collect::<Vec<_>>();
    let (done, failed, cancelled) = counts(&units);
    let terminal = matches!(
        task.job.state,
        JobState::Succeeded | JobState::Failed | JobState::Cancelled
    );
    let status = if task.job.state == JobState::Cancelled {
        "cancelled"
    } else if task.job.state == JobState::Failed && units.iter().all(|unit| unit.job.is_none()) {
        "failed"
    } else {
        aggregate(&units)
    };
    let mut value = json!({"task_id":task.job.id,"source":"native_import","task_kind":"processing","status":status,"current_step":task::step(Some(&task.job)),
        "total_chunks":units.len(),"done_chunks":done,"failed_chunks":failed,"cancelled_chunks":cancelled,"progress":if units.is_empty(){0.0}else{done as f64/units.len() as f64},
        "error":task.job.error.as_ref().map(|error|error.message.clone()).unwrap_or_default(),"file_count":task.inputs.len(),
        "created_at":task::stamp(&task.job.created_at)?,"updated_at":task::stamp(&task.job.updated_at)?,"started_at":null,"finished_at":if terminal{Some(task::stamp(&task.job.updated_at)?)}else{None},
        "schema_detected":"plain_text","artifact_paths":{},"rollback_info":{},"retry_parent_task_id":task.request.retry_parent_job_id,
        "retry_summary":if task.request.retry_parent_job_id.is_some(){json!({"chunk_retry_chunks":units.len(),"chunk_retry_files":task.inputs.len(),"file_fallback_files":0,"skipped_files":0,"parent_task_id":task.request.retry_parent_job_id})}else{json!({})},
        "cancel_requested_at":null,"cancel_origin":null,"unknown":{"started_at":"The task authority does not retain a separate start timestamp","cancel_origin":"The task authority retains cancellation state, not the legacy cancellation fields"},
        "native_request":task.request,"native_job":task.job});
    if detail {
        value["params"] = json!({"task_kind":"processing","llm_enabled":task.request.llm_enabled,"extract_time":task.request.extract_time,"vectors":task.request.vectors,"scope_type":if task.request.origin.as_ref().and_then(|origin|origin.conversation_ref.as_ref()).is_some(){"chat"}else{"global"},"chat_id":task.request.origin.as_ref().and_then(|origin|origin.conversation_ref.as_ref())});
        value["files"] = json!(
            task.inputs
                .iter()
                .map(|input| file(task, input, include_chunks))
                .collect::<Result<Vec<_>>>()?
        );
    }
    Ok(value)
}
