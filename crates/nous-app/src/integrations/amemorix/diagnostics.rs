use super::types::RuntimeInput;
use crate::App;
use nous_domain::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

pub(super) async fn stats(app: &App, ctx: &CallContext, subject: &SubjectId) -> Result<Value> {
    let report = app.subject_diagnostics(ctx, subject).await?;
    let backfill = backfill(&report);
    let count = |kind: &str| {
        report
            .inventory
            .cognition
            .get(kind)
            .map_or(0, |value| value.current)
    };
    let relations = report
        .inventory
        .cognition
        .get("association")
        .map_or(0, |value| value.current - value.deleted);
    let time = report
        .work
        .last_maintenance_at
        .as_deref()
        .map(OperationTime::parse)
        .transpose()?
        .map(|value| value.utc().timestamp_millis() as f64 / 1000.0);
    Ok(
        json!({"paragraphs":count("memory"),"relations":relations,"episodes":count("episode"),
        "profiles":null,"stale_paragraph_marks":null,"profile_refresh_pending":null,"profile_refresh_failed":null,
        "episode_rebuild_pending":null,"paragraph_vector_backfill_pending":backfill[0],"paragraph_vector_backfill_failed":backfill[2],
        "last_maintenance_at":time,"unknown":unknown(&report),"diagnostics":report}),
    )
}
pub(super) async fn runtime(
    app: &App,
    ctx: &CallContext,
    subject: &SubjectId,
    input: &RuntimeInput,
) -> Result<Value> {
    let action = input.action.trim().to_lowercase();
    if !matches!(action.as_str(), "get_config" | "list_vector_spaces") {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "runtime action requires its native mutation or provider-check owner",
        )
        .with_context("action", json!(input.action))
        .with_context(
            "available_actions",
            json!(["get_config", "list_vector_spaces"]),
        ));
    }
    if !input.parameters.is_empty() {
        return Err(Error::invalid(
            "read-only runtime actions accept no additional parameters",
        ));
    }
    let report = app.subject_diagnostics(ctx, subject).await?;
    let backfill = backfill(&report);
    if action == "list_vector_spaces" {
        let selected = report
            .runtime
            .pointer("/embedding_space/id")
            .and_then(Value::as_str);
        let items=report.vector_spaces.iter().map(|value|json!({"space_id":value.space.id,
            "embedding_fingerprint":{"model":value.space.provider_model,"dimension":value.space.dimension,"hash":value.space.id},
            "state":if selected==Some(value.space.id.as_str()) {"active"}else {"saved"},
            "can_delete":false,"vector_count":value.contents,"size_bytes":null,
            "counts":{"associations":value.associations,"pending_inputs":value.pending_inputs},"space":value.space,
            "unknown":{"size_bytes":"Vectors share SQLite storage; no per-space physical allocation owner exists."}})).collect::<Vec<_>>();
        return Ok(
            json!({"success":true,"items":items,"watermark":report.watermark,"state":if report.vector_spaces.is_empty() {"empty"}else {"ready"}}),
        );
    }
    let configured: Option<EmbeddingSpace> =
        serde_json::from_value(report.runtime["embedding_space"].clone())?;
    let stored = report
        .vector_spaces
        .iter()
        .filter(|value| value.contents > 0)
        .collect::<Vec<_>>();
    let dimensions = stored
        .iter()
        .map(|value| value.space.dimension)
        .collect::<BTreeSet<_>>();
    let matching = configured.as_ref().and_then(|space| {
        stored
            .iter()
            .find(|value| value.space.id == space.id)
            .copied()
    });
    let status = if configured.is_none() {
        "unknown"
    } else if matching.is_some() {
        "match"
    } else if stored.is_empty() {
        "missing"
    } else {
        "mismatched"
    };
    let model = &report.runtime["roles"][crate::retrieval::embedding_model::ROLE];
    let ready = model["resource_ready"].as_bool();
    let enabled = report
        .runtime
        .pointer("/configuration/integration/enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            Error::internal("A-Memorix enabled policy is missing from its configuration snapshot")
        })?;
    let mut available = vec!["metadata", "sparse"];
    let mut unavailable = Vec::new();
    if !stored.is_empty() {
        available.push("vector_read");
    }
    match ready {
        Some(true) => available.push("embedding"),
        Some(false) => unavailable.push("embedding"),
        None => {}
    }
    Ok(
        json!({"success":true,"config":report.runtime["configuration"],"data_dir":report.runtime["data_dir"],
        "embedding_dimension":configured.as_ref().map(|space|space.dimension),
        "stored_vector_dimension":if dimensions.len()==1 {dimensions.first().copied()}else {None},
        "embedding_fingerprint":configured,"stored_embedding_fingerprint":matching.map(|value|&value.space),
        "embedding_fingerprint_status":status,"vector_rebuild_required":configured.as_ref().map(|_|status=="mismatched"),
        "vector_rebuild_message":if status=="mismatched" {"Stored vectors belong to other immutable spaces; native embedding sync fills the configured space."}else {""},
        "auto_save":null,"relation_vectors_enabled":null,"vector_pools":null,
        "vector_pools_ready":null,"vector_pools_effective_mode":null,
        "memory_enabled":enabled,"runtime_ready":true,"retrieval_ready":enabled,
        "available_channels":available,"unavailable_channels":unavailable,
        "embedding_degraded":ready.map(|ready|!ready),"embedding_degraded_reason":model["readiness"]["reason"],
        "embedding_degraded_since":null,"embedding_last_check":null,
        "paragraph_vector_backfill_pending":backfill[0],"paragraph_vector_backfill_running":backfill[1],"paragraph_vector_backfill_failed":backfill[2],"paragraph_vector_backfill_done":backfill[3],
        "vector_spaces":report.vector_spaces,"models":report.models,"work":report.work,
        "unknown":unknown(&report),"watermark":report.watermark,
        "available_actions":["get_config","list_vector_spaces"]}),
    )
}

fn backfill(report: &SubjectDiagnostics) -> [Option<u64>; 4] {
    let Some(id) = report
        .runtime
        .pointer("/embedding_space/id")
        .and_then(Value::as_str)
    else {
        return [None; 4];
    };
    let states = report
        .vector_spaces
        .iter()
        .find(|value| value.space.id == id)
        .and_then(|value| value.input_states.get("memory"));
    let count = |state: &str| {
        states
            .and_then(|states| states.get(state))
            .copied()
            .unwrap_or(0)
    };
    [
        if count("unassigned") == 0 {
            Some(count("queued") + count("waiting"))
        } else {
            None
        },
        Some(count("running")),
        Some(count("failed")),
        Some(count("ready")),
    ]
}
fn unknown(report: &SubjectDiagnostics) -> BTreeMap<String, String> {
    let mut result = report.unknown.clone();
    if backfill(report).iter().any(Option::is_none) {
        result.insert("paragraph_vector_backfill".into(),"The configured embedding space or input's assigned task state is unavailable; native vector_spaces.input_states preserves the known facts.".into());
    }
    if report.runtime["data_dir"].is_null() {
        result.insert(
            "data_dir".into(),
            "The local instance directory requires administrator scope.".into(),
        );
    }
    result
}
