use nous_domain::*;
use nous_storage::OperationReceipt;
use serde_json::{Value, json};

pub(super) fn preview(plan: SourceTrashPreview) -> Value {
    let paragraphs = plan.memory_targets.len();
    let entities = plan.graph.retire_entities.len();
    let relations = plan.graph.retire_associations.len();
    let sources = plan.matched_sources.len();
    let item_count = paragraphs + entities + relations;
    let items = plan.memory_targets.iter().map(|reference|preview_item("paragraph",reference))
        .chain(plan.graph.retire_entities.iter().map(|reference|preview_item("entity",reference)))
        .chain(plan.graph.retire_associations.iter().map(|reference|preview_item("relation",reference)))
        .take(100).collect::<Vec<_>>();
    json!({"success":paragraphs>0,"mode":"source","selector":{"sources":plan.sources},
        "counts":{"paragraphs":paragraphs,"entities":entities,"relations":relations,"sources":sources,
            "requested_sources":plan.sources.len(),"matched_sources":sources},
        "requested_source_count":plan.sources.len(),"matched_source_count":sources,"sources":plan.matched_sources,
        "items":items,"item_count":item_count,"dry_run":true,"watermark":plan.watermark,
        "vector_associations_to_retire":plan.vector_associations_to_retire,
        "error":if paragraphs>0 {""} else {"No matching source records"},"native_plan":plan})
}
fn preview_item(kind:&str,reference:&CognitiveRef)->Value {
    json!({"item_type":kind,"item_hash":reference.id,"item_key":reference.to_string()})
}

pub(super) fn source_items(mut sources: Vec<NamedSourceSummary>) -> Vec<Value> {
    sources.sort_by(|left, right| {
        updated(right)
            .cmp(&updated(left))
            .then_with(|| left.source.cmp(&right.source))
    });
    sources.into_iter().map(source_item).collect()
}
fn updated(source: &NamedSourceSummary) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(&source.last_updated).ok()
}
fn source_item(source: NamedSourceSummary) -> Value {
    let last_updated = updated(&source).map(|time| time.timestamp_millis() as f64 / 1000.0);
    json!({"source":source.source,"count":source.memory_count,"paragraph_count":source.memory_count,
        "last_updated":last_updated,"material_count":source.material_count,"observation_count":source.observation_count,"protected_count":source.protected_count})
}
pub(super) fn deleted(receipt: OperationReceipt) -> Result<Value> {
    let native: TrashSourcesResult = serde_json::from_value(receipt.result.clone())?;
    let paragraphs = native.trashed_memories.len();
    let entities = native.graph.retired_entities.len();
    let relations = native.graph.retired_associations.len();
    let sources = native.matched_sources.len();
    let count = paragraphs + entities + relations + sources;
    Ok(
        json!({"success":count>0,"mode":"source","operation_id":receipt.commit_seq.to_string(),"delete_operation_id":receipt.commit_seq.to_string(),
        "counts":{"paragraphs":paragraphs,"entities":entities,"relations":relations,"sources":sources,"requested_sources":native.sources.len(),"matched_sources":sources},
        "sources":native.matched_sources,"deleted_count":count,"deleted_entity_count":entities,"deleted_relation_count":relations,
        "deleted_paragraph_count":paragraphs,"deleted_source_count":sources,"deleted_vector_count":native.retired_vector_associations,
        "requested_source_count":native.sources.len(),"matched_source_count":sources,"error":if count>0 {""} else {"No matching source records"},
        "native_result":native,"native_operation":receipt}),
    )
}
pub(super) fn restored(receipt: OperationReceipt, sources: Vec<String>) -> Result<Value> {
    let native: RestoreSourcesResult = serde_json::from_value(receipt.result.clone())?;
    Ok(
        json!({"success":true,"mode":"source","operation_id":native.trash_commit_seq.to_string(),
        "restored_paragraphs":native.restored_memories.iter().map(|reference|reference.id.as_str()).collect::<Vec<_>>(),
        "restored_entities":native.graph.restored_entities.iter().map(|reference|reference.id.as_str()).collect::<Vec<_>>(),
        "restored_entity_count":native.graph.restored_entities.len(),
        "restored_relations":native.graph.restored_associations.iter().map(|reference|reference.id.as_str()).collect::<Vec<_>>(),
        "restored_relation_count":native.graph.restored_associations.len(),"vector_associations_reused":native.vector_associations_reused,
        "relation_failures":[],"sources":sources,"native_result":native,"native_operation":receipt}),
    )
}
pub(super) fn operation(operation: nous_storage::SourceTrashOperation) -> Result<Value> {
    let native: TrashSourcesResult = serde_json::from_value(operation.trash.result.clone())?;
    let status = if operation.restore.is_some() {
        "restored"
    } else {
        "completed"
    };
    let counts = json!({"paragraphs":native.trashed_memories.len(),"entities":native.graph.retired_entities.len(),"relations":native.graph.retired_associations.len(),"sources":native.matched_sources.len(),"requested_sources":native.sources.len(),"matched_sources":native.matched_sources.len()});
    let items=native.trashed_memories.iter().map(|reference|json!({"item_type":"paragraph","item_hash":reference.id,"item_key":reference.to_string()}))
        .chain(native.graph.retired_entities.iter().map(|reference|json!({"item_type":"entity","item_hash":reference.id,"item_key":reference.to_string()})))
        .chain(native.graph.retired_associations.iter().map(|reference|json!({"item_type":"relation","item_hash":reference.id,"item_key":reference.to_string()}))).collect::<Vec<_>>();
    Ok(
        json!({"operation_id":operation.trash.commit_seq.to_string(),"mode":"source","status":status,
        "selector":{"sources":native.sources},"created_at":operation.trash.time.unix_ms()as f64/1000.0,
        "restored_at":operation.restore.as_ref().map(|receipt|receipt.time.unix_ms()as f64/1000.0),
        "reason":null,"requested_by":null,"actor":operation.trash.context.caller_id,
        "summary":{"state":status,"counts":counts,"sources":native.matched_sources},"items":items,
        "native_operation":operation.trash,"restore_receipt":operation.restore}),
    )
}
