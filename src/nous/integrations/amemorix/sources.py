"""Source administration projects authoritative previews, operations and lifecycle."""

from collections import defaultdict
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import CallContext, Contract, DomainError, SourceCommand, SourceResult
from pydantic import Field, JsonValue, TypeAdapter

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


class Selection(Contract):
    action: str
    mode: str = "source"
    source: str | None = None
    sources: tuple[str, ...] = ()
    selector: JsonValue = None
    query: str | None = None
    items: tuple[str, ...] = ()
    requested_by: str | None = Field(default=None, max_length=120)
    reason: str = "A-Memorix source management"
    operation_id: str | None = None
    request_id: str | None = None
    dry_run: bool = False
    force: bool = False
    cascade: bool = False
    grace_hours: float | None = None
    limit: int = Field(default=50, ge=1, le=1000)
    before_commit_seq: int | None = None


def source_label(source: dict[str, JsonValue]) -> str | None:
    from nous.material.named_sources import source_name

    return source_name(source)


async def source_admin(
    service: AMemorixService, ctx: CallContext, tool: str, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = Selection.model_validate(arguments)
    if command.mode.strip().lower() not in {"", "source"}:
        raise DomainError("capability_unavailable", "Deletion mode requires its scoped owner")
    if command.cascade or command.grace_hours is not None:
        raise DomainError(
            "capability_unavailable", "Source selection has no delayed cascade policy"
        )
    sources = [
        source for source in await service.app.sources.list(ctx) if not source.get("purged", False)
    ]
    grouped: dict[str, list[str]] = defaultdict(list)
    for source in sources:
        label = source_label(json_object(source))
        if label is not None:
            grouped[label].append(source["id"])
    if command.action == "list" and tool == "memory_source_admin":
        records = await service.records(ctx, ("memory",))
        items = [
            {
                "source": label,
                "paragraph_count": sum(
                    not record.deleted and bool(set(record.source_roots).intersection(ids))
                    for record in records
                ),
                "observation_ids": ids,
                "updated_at": max(
                    source["recorded_at"] for source in sources if source["id"] in ids
                ),
            }
            for label, ids in sorted(grouped.items())
        ]
        return json_object(
            {
                "success": True,
                "items": items,
                "count": len(items),
                "sources": items,
            }
        )
    if command.action in {"get_operation", "list_operations"}:
        if command.action == "get_operation":
            value = await selected_operation(service, ctx, command.operation_id)
            return json_object({"success": True, "operation": value})
        rows = await service.app.sources.list_operations(
            ctx, command.limit, before_commit_seq=command.before_commit_seq
        )
        return json_object(
            {
                "success": True,
                "operations": rows,
                "items": rows,
                "count": len(rows),
                "next_before_commit_seq": SourceResult.model_validate(rows[-1]["result"]).commit_seq
                if rows
                else None,
            }
        )
    labels = selected_labels(command)
    if command.action in {"restore", "purge"} and command.operation_id:
        raw = await selected_operation(service, ctx, command.operation_id)
        ids = SourceResult.model_validate(raw["result"]).observation_ids
    else:
        if not labels:
            raise DomainError(
                "source_selection_required", "Source administration requires explicit source labels"
            )
        missing = set(labels).difference(grouped)
        if missing:
            raise DomainError("source_not_found", "Selected source label is unavailable")
        ids = tuple(sorted({identity for label in labels for identity in grouped[label]}))
    preview = await service.app.sources.preview(ctx, ids)
    if command.action == "preview" or command.dry_run:
        return json_object(
            {
                "success": True,
                "preview": preview,
                "deleted_count": len(preview.retiring),
                "native_result": preview,
            }
        )
    operation = (
        command.request_id
        or (command.operation_id if command.action not in {"restore", "purge"} else None)
        or str(uuid4())
    )
    native = SourceCommand(
        operation_id=operation,
        observation_ids=ids,
        reason=command.reason.strip() or f"{tool}.{command.action}",
        requested_by=command.requested_by or tool,
        expected_preview=preview.preview_token,
    )
    if command.action in {"delete", "batch_delete", "execute"}:
        result = await service.app.sources.trash(ctx, native)
    elif command.action == "restore":
        result = await service.app.sources.restore(ctx, native)
    elif command.action == "purge":
        result = await service.app.sources.purge(ctx, native)
    else:
        raise DomainError("action_invalid", "Unknown source management action")
    return json_object(
        {
            "success": True,
            "operation_id": result.source_operation_id,
            "deleted_count": len(preview.retiring) if command.action != "restore" else 0,
            "native_result": result,
        }
    )


async def selected_operation(service: AMemorixService, ctx: CallContext, identity: str | None):
    if not identity:
        raise DomainError("operation_required", "Select a source operation")
    if identity.isdecimal():
        return await service.app.sources.get_operation(ctx, commit_seq=int(identity))
    return await service.app.sources.get_operation(ctx, identity)


def selected_labels(command: Selection) -> tuple[str, ...]:
    selector = command.selector
    if isinstance(selector, str):
        return (selector,)
    if isinstance(selector, list):
        return TypeAdapter(tuple[str, ...]).validate_python(selector)
    if selector is None:
        values = [*command.sources, *command.items]
        values.extend(value for value in (command.source, command.query) if value is not None)
        return tuple(sorted(set(values)))
    if not isinstance(selector, dict):
        raise DomainError("source_selection_invalid", "Source selector requires exact text labels")
    unknown = set(selector).difference({"source", "query", "sources", "items"})
    if unknown:
        raise DomainError("source_selection_invalid", "Unknown source selector fields")
    labels: list[str] = []
    for key in ("source", "query"):
        if key in selector:
            labels.append(TypeAdapter(str).validate_python(selector[key]))
    for key in ("sources", "items"):
        if key in selector:
            labels.extend(TypeAdapter(tuple[str, ...]).validate_python(selector[key]))
    return tuple(sorted(set(labels)))
