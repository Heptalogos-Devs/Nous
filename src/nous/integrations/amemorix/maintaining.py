"""Lifecycle tool selections freeze inside one authoritative, replayable operation."""

from typing import TYPE_CHECKING
from uuid import UUID

from nous_wave.amemorix.contracts import MaintainInput
from nous_wave.contracts import CallContext, CognitiveRecord, DomainError, MemoryBody, QueryRequest
from nous_wave.contracts.lifecycle import MemoryChanges
from pydantic import JsonValue, TypeAdapter

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def maintain(
    self: AMemorixService, ctx: CallContext, command: MaintainInput
) -> dict[str, JsonValue]:
    if command.action == "recycle_bin":
        records = await self.app.cognition.recycle_bin(ctx, limit=command.limit)
        return json_object(
            {
                "success": True,
                "count": len(records),
                "items": [
                    {
                        "hash": record.reference.id,
                        "content": record.body.text if isinstance(record.body, MemoryBody) else "",
                        "type": "paragraph",
                        "memory": record,
                    }
                    for record in records
                ],
            }
        )
    if command.hours is not None and command.action != "protect":
        raise DomainError("invalid_memory_hours", "Hours applies to protection")
    async with self.app.authority.write(
        ctx,
        command.operation_id,
        operation_kind="amemorix.maintain",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            saved = json_object(tx.replay_result)
            for record in TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
                saved.get("memories", [])
            ):
                if not record.purged:
                    await self.app.cognition.get_in(tx, ctx, record.reference)
            return saved
        target = command.target.strip()
        direct = ":" in target
        if not direct:
            try:
                UUID(target)
            except ValueError:
                pass
            else:
                direct = True
        if direct:
            references = (await self.memory_ref(ctx, target),)
        elif target:
            result = await self.app.retrieval.query(
                ctx,
                QueryRequest(
                    text=target,
                    mode="lexical",
                    kinds=("memory",),
                    limit=command.limit,
                    include_deleted=command.action == "restore",
                    include_inactive=True,
                ),
            )
            references = tuple(
                hit.object.reference
                for hit in result.hits
                if isinstance(hit.object, CognitiveRecord)
            )
        else:
            references = ()
        if not references:
            return tx.result({"success": False, "detail": "no_matching_target", "targets": []})
        changed = await self.app.cognition.change_memories_in(
            tx,
            ctx,
            MemoryChanges(
                operation_id=command.operation_id,
                references=references,
                action=command.action,
                strength=command.strength,
                hours=command.hours,
                reason=command.reason,
                force=command.force,
            ),
        )
        result = tx.result(
            json_object(
                {
                    "success": True,
                    "detail": command.action,
                    "count": len(changed),
                    "targets": [record.reference for record in changed],
                    "memories": changed,
                    "memory": changed[0] if len(changed) == 1 else None,
                    "reference": changed[0].reference if len(changed) == 1 else None,
                }
            )
        )
    if command.action == "purge":
        await self.app.retrieval.invalidate()
    return result
