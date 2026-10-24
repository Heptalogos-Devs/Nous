//! Bounded trace diagnostics. Membership uses a fixed cutoff; mutable states use one current read view.
use crate::{Database, codec::*, storage_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use sqlx::{Row, sqlite::SqliteRow};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceCursor {
    pub trace_id: TraceId,
    pub snapshot_seq: i64,
    pub operations_after: i64,
    pub jobs_after: Option<String>,
    pub calls_after: Option<ModelCallId>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceOperation {
    pub caller_id: String,
    pub operation: String,
    pub scope: String,
    pub operation_id: String,
    pub commit_seq: i64,
    pub job_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceJob {
    pub id: String,
    pub accepted_commit_seq: i64,
    pub namespace_id: Option<NamespaceId>,
    pub subject_id: Option<SubjectId>,
    pub kind: String,
    pub state: JobState,
    pub dependencies: Vec<String>,
    pub waiting_for: Option<String>,
    pub error: Option<Error>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceModelCall {
    pub id: ModelCallId,
    pub job_id: String,
    pub reservation_commit_seq: i64,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub step_key: String,
    pub attempt: i64,
    pub resource_id: String,
    pub state: ModelCallState,
    pub provider_model: Option<String>,
    pub usage: Option<serde_json::Value>,
    pub cost_microunits: Option<i64>,
    pub error: Option<Error>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceRecords {
    pub trace_id: TraceId,
    pub snapshot_seq: i64,
    /// Job and call states reflect this read view, including updates after the membership cutoff.
    pub read_at_commit_seq: i64,
    pub operations: Vec<TraceOperation>,
    pub jobs: Vec<TraceJob>,
    pub model_calls: Vec<TraceModelCall>,
    pub next_cursor: TraceCursor,
    pub operations_has_more: bool,
    pub jobs_has_more: bool,
    pub calls_has_more: bool,
}

impl Database {
    pub async fn trace_records(
        &self,
        trace_id: &TraceId,
        cursor: Option<&TraceCursor>,
        limit: usize,
    ) -> Result<TraceRecords> {
        trace_id.validate()?;
        if !(1..=500).contains(&limit) {
            return Err(Error::invalid("trace page limit must be 1..500"));
        }
        let mut view = self.read_view().await?;
        let mut next = cursor.cloned().unwrap_or_else(|| TraceCursor {
            trace_id: trace_id.clone(),
            snapshot_seq: view.watermark,
            operations_after: 0,
            jobs_after: None,
            calls_after: None,
        });
        if next.trace_id != *trace_id
            || next.snapshot_seq < 0
            || next.snapshot_seq > view.watermark
            || next.operations_after < 0
            || next.operations_after > next.snapshot_seq
        {
            return Err(Error::invalid(
                "trace cursor does not belong to this committed trace view",
            ));
        }
        if let Some(id) = &next.jobs_after {
            super::tasks::validate_job_id(id)?;
        }
        if let Some(id) = &next.calls_after {
            id.validate()?;
        }
        let fetch_limit = (limit + 1) as i64;
        let rows = sqlx::query("SELECT caller_id,operation,scope,operation_id,commit_seq,json_extract(receipt,'$.job_ids') AS job_ids FROM operation_receipts WHERE trace_id=? AND commit_seq>? AND commit_seq<=? ORDER BY commit_seq LIMIT ?")
            .bind(trace_id.as_str()).bind(next.operations_after).bind(next.snapshot_seq).bind(fetch_limit)
            .fetch_all(&mut *view.tx).await.map_err(storage_error)?;
        let operations_has_more = rows.len() > limit;
        let operations = rows
            .into_iter()
            .take(limit)
            .map(operation_row)
            .collect::<Result<Vec<_>>>()?;
        let rows = sqlx::query("SELECT job_id,accepted_commit_seq,namespace_id,subject_id,kind,state,dependencies,json_extract(wait,'$.kind') AS waiting_for,error,created_at,updated_at FROM task_records WHERE trace_id=? AND accepted_commit_seq<=? AND (? IS NULL OR job_id>?) ORDER BY job_id LIMIT ?")
            .bind(trace_id.as_str()).bind(next.snapshot_seq).bind(&next.jobs_after).bind(&next.jobs_after).bind(fetch_limit)
            .fetch_all(&mut *view.tx).await.map_err(storage_error)?;
        let jobs_has_more = rows.len() > limit;
        let jobs = rows
            .into_iter()
            .take(limit)
            .map(job_row)
            .collect::<Result<Vec<_>>>()?;
        let rows = sqlx::query("SELECT id,job_id,reservation_commit_seq,namespace_id,subject_id,step_key,attempt,resource_id,state,json_extract(response,'$.record.provider_model') AS provider_model,json_extract(response,'$.record.usage') AS usage,cost_microunits,error,created_at,completed_at FROM model_calls WHERE trace_id=? AND reservation_commit_seq<=? AND (? IS NULL OR id>?) ORDER BY id LIMIT ?")
            .bind(trace_id.as_str()).bind(next.snapshot_seq).bind(next.calls_after.as_ref().map(ModelCallId::as_str)).bind(next.calls_after.as_ref().map(ModelCallId::as_str)).bind(fetch_limit)
            .fetch_all(&mut *view.tx).await.map_err(storage_error)?;
        let calls_has_more = rows.len() > limit;
        let model_calls = rows
            .into_iter()
            .take(limit)
            .map(call_row)
            .collect::<Result<Vec<_>>>()?;
        if let Some(row) = operations.last() {
            next.operations_after = row.commit_seq;
        }
        if let Some(row) = jobs.last() {
            next.jobs_after = Some(row.id.clone());
        }
        if let Some(row) = model_calls.last() {
            next.calls_after = Some(row.id.clone());
        }
        let result = TraceRecords {
            trace_id: trace_id.clone(),
            snapshot_seq: next.snapshot_seq,
            read_at_commit_seq: view.watermark,
            operations,
            jobs,
            model_calls,
            next_cursor: next,
            operations_has_more,
            jobs_has_more,
            calls_has_more,
        };
        view.finish().await?;
        Ok(result)
    }
}

fn operation_row(row: SqliteRow) -> Result<TraceOperation> {
    Ok(TraceOperation {
        caller_id: row.get("caller_id"),
        operation: row.get("operation"),
        scope: row.get("scope"),
        operation_id: row.get("operation_id"),
        commit_seq: row.get("commit_seq"),
        job_ids: serde_json::from_str(row.get("job_ids"))?,
    })
}

fn job_row(row: SqliteRow) -> Result<TraceJob> {
    Ok(TraceJob {
        id: row.get("job_id"),
        accepted_commit_seq: row.get("accepted_commit_seq"),
        namespace_id: row
            .get::<Option<String>, _>("namespace_id")
            .map(NamespaceId),
        subject_id: row.get::<Option<String>, _>("subject_id").map(SubjectId),
        kind: row.get("kind"),
        state: parse_enum(row.get("state"))?,
        dependencies: serde_json::from_str(row.get("dependencies"))?,
        waiting_for: row.get("waiting_for"),
        error: parse_optional(row.get("error"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn call_row(row: SqliteRow) -> Result<TraceModelCall> {
    Ok(TraceModelCall {
        id: ModelCallId(row.get("id")),
        job_id: row.get("job_id"),
        reservation_commit_seq: row.get("reservation_commit_seq"),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        step_key: row.get("step_key"),
        attempt: row.get("attempt"),
        resource_id: row.get("resource_id"),
        state: parse_enum(row.get("state"))?,
        provider_model: row.get("provider_model"),
        usage: parse_optional(row.get("usage"))?,
        cost_microunits: row.get("cost_microunits"),
        error: parse_optional(row.get("error"))?,
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    })
}
