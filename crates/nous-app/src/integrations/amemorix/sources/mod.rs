mod input;
mod projection;
use super::AmemorixRequest;
use crate::App;
pub use input::{DeleteAdminInput, SourceAdminInput};
use nous_domain::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
struct Audit<'a> {
    action: &'a str,
    requested_by: &'a str,
    reason: &'a str,
}

pub(super) async fn execute(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &SourceAdminInput,
) -> Result<Value> {
    let action = input.action.trim().to_ascii_lowercase();
    match action.as_str() {
        "list" => {
            let directory = app
                .source_directory(
                    ctx,
                    &SourceDirectoryRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                    },
                )
                .await?;
            let items = projection::source_items(directory.items);
            Ok(
                json!({"success":true,"count":items.len(),"items":items,"watermark":directory.watermark}),
            )
        }
        "delete" | "batch_delete" => {
            let sources = if action == "delete" {
                vec![input.source.clone().ok_or_else(|| {
                    Error::invalid("source deletion requires its exact source label")
                })?]
            } else {
                input.sources.clone()
            };
            let requested_by = if input.requested_by.is_empty() {
                "memory_source_admin"
            } else {
                &input.requested_by
            };
            let reason = if input.reason.is_empty() {
                if action == "delete" {
                    "source_delete"
                } else {
                    "source_batch_delete"
                }
            } else {
                &input.reason
            };
            trash(
                app,
                ctx,
                binding,
                sources,
                input.operation_id.as_deref(),
                Audit {
                    action: &format!("memory_source_admin.{action}"),
                    requested_by,
                    reason,
                },
            )
            .await
        }
        _ => Err(unavailable("memory_source_admin", &action, "")),
    }
}
pub(super) async fn delete_admin(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &DeleteAdminInput,
) -> Result<Value> {
    let action = input.action.trim().to_ascii_lowercase();
    let mode = input.mode.trim().to_ascii_lowercase();
    if !mode.is_empty() && mode != "source" {
        return Err(unavailable("memory_delete_admin", &action, &mode));
    }
    match action.as_str() {
        "preview" | "execute" if mode == "source" && (action == "preview" || input.dry_run) => {
            let preview = app
                .preview_sources(
                    ctx,
                    TrashSourcesRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        sources: source_selection(input)?,
                        force: true,
                        reason: audit_reason(Audit {
                            action: "memory_delete_admin.preview",
                            requested_by: &input.requested_by,
                            reason: &input.reason,
                        }),
                    },
                )
                .await?;
            Ok(projection::preview(preview))
        }
        "execute" if mode == "source" => {
            let sources = source_selection(input)?;
            trash(
                app,
                ctx,
                binding,
                sources,
                input.request_id.as_deref(),
                Audit {
                    action: "memory_delete_admin.execute",
                    requested_by: &input.requested_by,
                    reason: &input.reason,
                },
            )
            .await
        }
        "restore" => {
            let trash_commit_seq = source_operation(&input.operation_id)?;
            let original = app
                .source_trash_operation(
                    ctx,
                    &SourceOperationRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        commit_seq: trash_commit_seq,
                    },
                )
                .await?;
            let trashed: TrashSourcesResult = serde_json::from_value(original.trash.result)?;
            let operation_id = operation_id(input.request_id.as_deref());
            let receipt = app
                .restore_sources(
                    ctx,
                    &operation_id,
                    RestoreSourcesRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        trash_commit_seq,
                        reason: audit_reason(Audit {
                            action: "memory_delete_admin.restore",
                            requested_by: &input.requested_by,
                            reason: &input.reason,
                        }),
                    },
                )
                .await?;
            projection::restored(receipt, trashed.matched_sources)
        }
        "get_operation" => {
            let operation = match app
                .source_trash_operation(
                    ctx,
                    &SourceOperationRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        commit_seq: source_operation(&input.operation_id)?,
                    },
                )
                .await
            {
                Ok(operation) => operation,
                Err(error) if error.code == ErrorCode::NotFound => {
                    return Ok(json!({"success":false,"operation":null,"error":error.message}));
                }
                Err(error) => return Err(error),
            };
            Ok(json!({"success":true,"operation":projection::operation(operation)?}))
        }
        "list_operations" => {
            let page = app
                .source_trash_operations(
                    ctx,
                    &SourceOperationsRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        limit: input.limit.clamp(1, 200),
                        before_commit_seq: input.before_commit_seq,
                    },
                )
                .await?;
            let items = page
                .items
                .into_iter()
                .map(projection::operation)
                .collect::<Result<Vec<_>>>()?;
            Ok(
                json!({"success":true,"count":items.len(),"items":items,"next_before_commit_seq":page.next_before_commit_seq,"watermark":page.watermark}),
            )
        }
        _ => Err(unavailable("memory_delete_admin", &action, &mode)),
    }
}
async fn trash(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    sources: Vec<String>,
    identity: Option<&str>,
    audit: Audit<'_>,
) -> Result<Value> {
    let operation_id = operation_id(identity);
    let receipt = app
        .trash_sources(
            ctx,
            &operation_id,
            TrashSourcesRequest {
                namespace_id: binding.namespace_id.clone(),
                subject_id: binding.subject_id.clone(),
                sources,
                force: true,
                reason: audit_reason(audit),
            },
        )
        .await?;
    projection::deleted(receipt)
}
fn operation_id(value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| ulid::Ulid::generate().to_string())
}
fn source_operation(value: &str) -> Result<i64> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|seq| *seq > 0)
        .ok_or_else(|| {
            Error::invalid(
                "source deletion operation_id must be the exact positive trash receipt commit_seq",
            )
        })
}
fn audit_reason(audit: Audit<'_>) -> String {
    format!(
        "{} {}",
        audit.action,
        json!({"requested_by":audit.requested_by,"reason":audit.reason})
    )
}
fn unavailable(tool: &str, action: &str, mode: &str) -> Error {
    Error::new(
        ErrorCode::CapabilityUnavailable,
        "delete action requires its native scoped selection owner",
    )
    .with_context("tool", tool)
    .with_context("action", action)
    .with_context("mode", mode)
}
fn source_selection(input: &DeleteAdminInput) -> Result<Vec<String>> {
    let mut selected = BTreeSet::new();
    if let Some(selector) = &input.selector {
        if let Value::String(source) = selector {
            selected.insert(source.clone());
            return Ok(selected.into_iter().collect());
        }
        if let Value::Array(items) = selector {
            for item in items {
                selected.insert(
                    item.as_str()
                        .ok_or_else(|| Error::invalid("source selector label must be text"))?
                        .into(),
                );
            }
            return Ok(selected.into_iter().collect());
        }
        let object = selector
            .as_object()
            .ok_or_else(|| Error::invalid("source selector must be text, an array or an object"))?;
        for key in ["source", "query"] {
            if let Some(value) = object.get(key) {
                selected.insert(
                    value
                        .as_str()
                        .ok_or_else(|| Error::invalid("source selector label must be text"))?
                        .into(),
                );
            }
        }
        for key in ["sources", "items"] {
            if let Some(values) = object.get(key) {
                for value in values.as_array().ok_or_else(|| {
                    Error::invalid("source selector must contain an array of exact labels")
                })? {
                    selected.insert(
                        value
                            .as_str()
                            .ok_or_else(|| Error::invalid("source selector label must be text"))?
                            .into(),
                    );
                }
            }
        }
    } else {
        if let Some(source) = &input.source {
            selected.insert(source.clone());
        }
        if let Some(source) = &input.query {
            selected.insert(source.clone());
        }
        selected.extend(input.sources.iter().chain(input.items.iter()).cloned());
    }
    Ok(selected.into_iter().collect())
}
