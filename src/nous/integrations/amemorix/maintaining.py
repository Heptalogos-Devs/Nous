"""Lifecycle tool selections freeze inside one authoritative, replayable operation."""

from nous_sdk.presentation import body_text

import re
from typing import TYPE_CHECKING
from uuid import UUID, uuid4

from nous_sdk.amemorix.contracts import MaintainInput, V5AdminInput
from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    QueryRequest,
    RetentionChanges,
)
from nous_sdk.contracts.common import utc_now
from nous_sdk.contracts.retention import DecayRetention
from pydantic import JsonValue, TypeAdapter

from nous.cognition.lifecycle import (
    FREEZE_THRESHOLD,
    HALF_LIFE_HOURS,
    is_active,
    retention,
    snapshot,
    state_for,
)

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def selected_references(
    self: AMemorixService, ctx: CallContext, command: MaintainInput
) -> tuple[CognitiveRef, ...]:
    target = command.target.strip()
    direct = ":" in target or bool(re.fullmatch(r"[0-9a-fA-F]{32,64}", target))
    if not direct:
        try:
            UUID(target)
        except ValueError:
            pass
        else:
            direct = True
    if direct:
        return (await self.retention_ref(ctx, target),)
    if not target:
        return ()
    result = await self.app.retrieval.query(
        ctx,
        QueryRequest(
            text=target,
            mode="lexical",
            kinds=("memory", "association"),
            limit=command.limit,
            include_deleted=command.action in {"restore", "purge"},
            include_inactive=True,
        ),
    )
    return tuple(
        hit.object.reference for hit in result.hits if isinstance(hit.object, CognitiveRecord)
    )


async def maintain(
    self: AMemorixService, ctx: CallContext, command: MaintainInput
) -> dict[str, JsonValue]:
    if command.action == "decay" and not command.target.strip():
        result = await self.app.cognition.decay_retention(ctx, DecayRetention(operation_id=command.operation_id, limit=command.limit, cursor=command.cursor))
        return json_object({"success": True, "detail": "decay", "result": result, "targets": result.changed,
            "count": len(result.changed), "next_cursor": result.next_cursor, "complete": result.complete})
    if command.action == "recycle_bin":
        records = tuple(record for record in await self.records(ctx, ("memory", "association"), purpose="management")
            if not record.purged and (record.deleted or not is_active(record)))[:command.limit]
        return json_object(
            {
                "success": True,
                "count": len(records),
                "items": [
                    {
                        "hash": record.reference.id,
                        "content": body_text(record.body),
                        "type": "paragraph" if record.reference.kind == "memory" else "relation",
                        "record": record,
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
        scope=ctx.namespace_id,
        payload=command,
    ) as tx:
        if tx.replayed:
            saved = json_object(tx.replay_result)
            for record in TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
                saved.get("records", saved.get("memories", []))
            ):
                if not record.purged:
                    await self.app.cognition.get_in(tx, ctx, record.reference)
            return saved
        references = await selected_references(self, ctx, command)
        if not references:
            return tx.result({"success": False, "detail": "no_matching_target", "targets": []})
        changed = await self.app.cognition.change_retentions_in(
            tx,
            ctx,
            RetentionChanges(
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
                    "records": changed,
                    "memories": tuple(record for record in changed if record.reference.kind == "memory"),
                    "relations": tuple(record for record in changed if record.reference.kind == "association"),
                    "memory": changed[0] if len(changed) == 1 and changed[0].reference.kind == "memory" else None,
                    "record": changed[0] if len(changed) == 1 else None,
                    "reference": changed[0].reference if len(changed) == 1 else None,
                }
            )
        )
    await self.app.retrieval.invalidate()
    return result


async def v5_admin(
    self: AMemorixService, ctx: CallContext, command: V5AdminInput
) -> dict[str, JsonValue]:
    operation_id = command.operation_id or str(uuid4())
    if command.action == "status":
        records = tuple(
            record
            for record in await self.records(ctx, ("memory", "association"), purpose="management")
            if not record.purged
        )
        now = utc_now().coordinate
        live = tuple(record for record in records if not record.deleted)
        result = json_object(
            {
                "success": True,
                "active_count": sum(is_active(record) for record in live),
                "inactive_count": sum(not is_active(record) for record in live),
                "deleted_count": sum(record.deleted for record in records),
                "pinned_count": sum(
                    bool(record.lifecycle and record.lifecycle.pinned) for record in live
                ),
                "permanent_count": sum(state_for(record).permanent for record in live),
                "temp_protected_count": sum(
                    bool(record.lifecycle and (record.lifecycle.protected_until_unix_ms or 0) > now)
                    for record in live
                ),
                "config": {"half_life_hours": HALF_LIFE_HOURS, "prune_threshold": FREEZE_THRESHOLD},
                "last_maintenance_at": None,
            }
        )
        if command.target:
            references = await selected_references(
                self,
                ctx,
                MaintainInput(
                    action="restore",
                    target=command.target,
                    limit=command.limit,
                    operation_id=operation_id,
                ),
            )
            selected = {reference.id for reference in references}
            items: list[dict[str, JsonValue]] = []
            for record in records:
                if record.reference.id not in selected:
                    continue
                lifecycle = state_for(record)
                actual = snapshot(record, now)
                items.append(
                    json_object(
                        {
                            "hash": record.reference.id,
                            "content": body_text(record.body),
                            "type": "paragraph" if record.reference.kind == "memory" else "relation",
                            "lifecycle_revision": record.reference.revision,
                            "retention_strength": actual.strength,
                            "inactive_since": actual.inactive_since_unix_ms / 1000 if actual.inactive_since_unix_ms is not None else None,
                            "inactive_reason": actual.inactive_reason,
                            "is_inactive": actual.inactive,
                            "is_permanent": actual.permanent,
                            "state": "deleted"
                            if record.deleted
                            else "active"
                            if is_active(record)
                            else "inactive",
                            "is_pinned": bool(lifecycle and lifecycle.pinned),
                            "temp_protected": bool(
                                lifecycle and (lifecycle.protected_until_unix_ms or 0) > now
                            ),
                            "protected_until": lifecycle.protected_until_unix_ms / 1000
                            if lifecycle and lifecycle.protected_until_unix_ms
                            else None,
                            "last_reinforced": lifecycle.last_reinforced_unix_ms / 1000
                            if lifecycle and lifecycle.last_reinforced_unix_ms
                            else None,
                            "retention_score": retention(lifecycle, now) if lifecycle else None,
                            "retention_anchor_at": lifecycle.anchor_unix_ms / 1000
                            if lifecycle
                            else None,
                            "reference": record.reference,
                            "record": record,
                        }
                    )
                )
            result.update(
                json_object(
                    {
                        "items": items[: command.limit],
                        "count": min(len(items), command.limit),
                        "target": command.target,
                    }
                )
            )
        return result
    action = "protect" if command.action == "remember_forever" else command.action
    native = MaintainInput(
        action=action,
        target=command.target,
        strength=command.strength,
        limit=command.limit,
        reason=command.reason,
        operation_id=operation_id,
    )
    if command.action != "remember_forever":
        return await maintain(self, ctx, native)
    async with self.app.authority.write(
        ctx,
        operation_id,
        operation_kind="amemorix.maintain",
        scope=ctx.namespace_id,
        payload=command,
    ) as tx:
        if tx.replayed:
            saved = json_object(tx.replay_result)
            await self.app.cognition.replay_records_in(
                tx,
                ctx,
                TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(saved.get("records", saved.get("memories", []))),
            )
            return saved
        references = await selected_references(self, ctx, native)
        if not references:
            return tx.result({"success": False, "detail": "no_matching_target", "targets": []})
        changed = await self.app.cognition.change_retentions_in(
            tx, ctx, RetentionChanges(operation_id=operation_id, references=references,
                action="remember_forever", strength=command.strength, reason=command.reason),
        )
        result = tx.result(json_object({"success": True, "detail": command.action,
            "count": len(changed), "hashes": [record.reference.id for record in changed],
            "targets": [record.reference for record in changed], "records": changed,
            "memories": tuple(record for record in changed if record.reference.kind == "memory"),
            "relations": tuple(record for record in changed if record.reference.kind == "association")}))
    await self.app.retrieval.invalidate()
    return result
