"""Source administration projects authoritative previews, operations and lifecycle."""

from collections import defaultdict
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import CallContext, Contract, DomainError, SourceCommand, SourceResult
from pydantic import Field, JsonValue, TypeAdapter

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


class Selection(Contract):
    action: str
    mode: str = "source"
    source: str | None = None
    sources: tuple[str, ...] = ()
    selector: dict[str, JsonValue] = Field(default_factory=dict)
    reason: str = "A-Memorix source management"
    operation_id: str | None = None
    request_id: str | None = None
    dry_run: bool = False
    force: bool = False
    limit: int = Field(default=50, ge=1, le=1000)
    before_commit_seq: int | None = None


def source_label(source: dict[str, JsonValue]) -> str:
    kind = source.get("source_type")
    origin = source.get("origin")
    chat = origin.get("conversation_ref") if isinstance(origin, dict) else None
    return f"{kind}:{chat}" if isinstance(chat, str) and chat else str(kind)


async def source_admin(
    service: AMemorixService, ctx: CallContext, tool: str, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = Selection.model_validate(arguments)
    sources = await service.app.sources.list(ctx)
    grouped: dict[str, list[str]] = defaultdict(list)
    for source in sources:
        grouped[source_label(json_object(source))].append(source["id"])
    if command.action == "list" and tool == "memory_source_admin":
        records = await service.records(ctx, ("memory",))
        return json_object(
            {
                "success": True,
                "sources": [
                    {
                        "source": label,
                        "paragraph_count": sum(
                            bool(set(record.source_roots).intersection(ids)) for record in records
                        ),
                        "observation_ids": ids,
                    }
                    for label, ids in sorted(grouped.items())
                ],
            }
        )
    if command.action in {"get_operation", "list_operations"}:
        if command.action == "get_operation":
            if not command.operation_id:
                raise DomainError("operation_required", "Select a source operation")
            value = await service.app.sources.get_operation(ctx, command.operation_id)
            return json_object({"success": True, "operation": value})
        return json_object(
            {
                "success": True,
                "operations": await service.app.sources.list_operations(ctx, command.limit),
            }
        )
    labels = command.sources or (
        (command.source,)
        if command.source
        else TypeAdapter(tuple[str, ...]).validate_python(command.selector.get("sources", ()))
    )
    if command.action in {"restore", "purge"} and command.operation_id:
        raw = await service.app.sources.get_operation(ctx, command.operation_id)
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
    operation = command.request_id or command.operation_id or str(uuid4())
    native = SourceCommand(
        operation_id=operation,
        observation_ids=ids,
        reason=command.reason,
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
