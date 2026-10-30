"""Memory operations share the cognitive authority transaction."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts.lifecycle import MemoryChanges
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.identity.service import require_subject_in
from nous.platform.persistence import WriteTx

from .projection import body_text
from .schema import revisions, vectors

if TYPE_CHECKING:
    from .service import CognitionService

from nous_wave.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    DomainError,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    MemoryChange,
    MemoryLifecycle,
    RevisionCause,
)


async def change_memory_in(
    self: CognitionService,
    tx: WriteTx,
    ctx: CallContext,
    command: MemoryChange,
    *,
    revision_source: EvidenceLink | None = None,
) -> CognitiveRecord:
    from .lifecycle import transition

    previous = await self.get_in(tx, ctx, command.reference)
    if (
        previous.reference.kind != "memory"
        or previous.purged
        or not isinstance(previous.body, MemoryBody)
    ):
        raise DomainError("memory_unavailable", "Maintenance requires a nonpurged Memory revision")
    if previous.deleted and command.action not in {"restore", "purge"}:
        raise DomainError("memory_deleted", "Restore the memory before changing it")
    if command.body is not None and command.action != "revise":
        raise DomainError("invalid_memory_edit", "A body edit is accepted only for revise")
    if command.action == "revise" and command.body is None:
        raise DomainError("invalid_memory_edit", "Revise requires the complete edited memory body")
    if command.action == "revise" and revision_source is None:
        raise DomainError("memory_revision_source_required", "Admit the actual edited input first")
    now = int(__import__("datetime").datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
    lifecycle = previous.lifecycle or MemoryLifecycle(
        anchor_unix_ms=previous.times.recorded.coordinate
    )
    if command.action in {"trash", "purge"} and lifecycle.protected(now) and not command.force:
        raise DomainError(
            "memory_protected", "Memory is protected; unprotect or explicitly force the change"
        )
    result = await self.adopt_in(
        tx,
        ctx,
        AdoptCognition(
            id=previous.reference.id,
            expected_revision=command.reference.revision,
            body=command.body or previous.body,
            evidence=(revision_source,)
            if revision_source is not None
            else (
                EvidenceLink(reference=previous.reference, role=command.reason or command.action),
            ),
            times=previous.times,
            producer_call_id=None if command.action == "revise" else previous.producer_call_id,
        ),
    )
    result = result.model_copy(
        update={
            "lifecycle": transition(lifecycle, command, now),
            "deleted": command.action in {"trash", "purge"}
            if command.action in {"trash", "purge", "restore"}
            else previous.deleted,
            "cause": RevisionCause(
                action=command.action,
                actor=ctx.caller_id,
                reason=command.reason,
                parent=previous.reference,
            ),
        }
    )
    if command.action == "purge":
        result = result.model_copy(update={"body": None, "purged": True, "deleted": True})
        history = [
            CognitiveRecord.model_validate(value)
            for value in (
                await tx.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.id == previous.reference.id,
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.subject_id == ctx.subject_id,
                    )
                )
            ).scalars()
        ]
        for historical in history:
            tombstone = historical.model_copy(
                update={"body": None, "purged": True, "deleted": True}
            )
            await tx.connection.execute(
                update(revisions)
                .where(
                    revisions.c.id == historical.reference.id,
                    revisions.c.revision == historical.reference.revision,
                )
                .values(record=tombstone.model_dump(mode="json"))
            )
        references = tuple(
            record.reference for record in history
        ) + await self.retire_unsupported_in(tx, ctx)
        await tx.connection.execute(
            vectors.delete().where(vectors.c.reference.in_([ref.exact() for ref in references]))
        )
        if self.purge_context_in:
            await self.purge_context_in(tx, ctx, references)

        def contains_body(value: JsonValue) -> bool:
            if isinstance(value, list):
                return any(contains_body(child) for child in value)
            if not isinstance(value, dict):
                return False
            reference = value.get("reference")
            return (
                isinstance(reference, dict)
                and reference.get("id") == previous.reference.id
                and "body" in value
            ) or any(contains_body(child) for child in value.values())

        await self.authority.redact_receipts_in(tx, contains_body)
    elif body_text(result.body) == body_text(previous.body):
        assets = (
            (
                await tx.connection.execute(
                    select(vectors).where(vectors.c.reference == previous.reference.exact())
                )
            )
            .mappings()
            .all()
        )
        for asset in assets:
            await tx.connection.execute(
                vectors.insert().values(
                    **{
                        **asset,
                        "reference": result.reference.exact(),
                        "commit_seq": tx.commit_seq,
                    }
                )
            )
    await tx.connection.execute(
        update(revisions)
        .where(
            revisions.c.id == result.reference.id,
            revisions.c.revision == result.reference.revision,
        )
        .values(record=result.model_dump(mode="json"))
    )
    return result


async def change_memory(
    self: CognitionService, ctx: CallContext, command: MemoryChange
) -> CognitiveRecord:
    document = (
        ImportDocument(
            text=command.model_dump_json(),
            format="json",
            filename="memory-revision.json",
            media_type="application/json",
            source_type="administrative_change",
            source="memory_management",
            origin={"caller_id": ctx.caller_id},
        )
        if command.action == "revise"
        else None
    )
    store = self.materials.content_store
    lease = (
        await store.publish_bytes(document.inline_text().encode(), document.media_type)
        if document is not None and store
        else None
    )
    try:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="memory.change",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                result = CognitiveRecord.model_validate(tx.replay_result)
                if document is not None and ctx.source_groups():
                    admission = await self.materials.restore_input_in(
                        tx,
                        ctx,
                        operation_id=command.operation_id,
                        operation_kind="memory.change",
                        scope=ctx.require_subject(),
                    )
                    effective = await self.qualify_known_outputs_in(
                        tx,
                        ctx,
                        (result.reference,),
                        operation_id=command.operation_id,
                        operation_kind="memory.change",
                        scope=ctx.require_subject(),
                        input_material_ids=admission.material_ids,
                    )
                else:
                    effective = ctx
                return await self.replay_record_in(tx, effective, result)
            effective, revision_source = ctx, None
            if document is not None:
                await self.get_in(tx, ctx, command.reference)
                admission = await self.materials.admit_text_in(tx, ctx, document, lease)
                effective = admission.context
                revision_source = EvidenceLink(
                    observation_id=admission.event["id"], role="memory_revision"
                )
            result = tx.result(
                await change_memory_in(
                    self, tx, effective, command, revision_source=revision_source
                )
            )
    finally:
        if lease and store:
            await store.release(lease)
    if self.invalidate:
        await self.invalidate()
    return result


async def recycle_bin(
    self: CognitionService, ctx: CallContext, limit: int = 50
) -> tuple[CognitiveRecord, ...]:
    if not 1 <= limit <= 1000:
        raise DomainError("invalid_limit", "Recycle-bin limit must be between 1 and 1000")
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx, "memory")
        result: list[CognitiveRecord] = []
        roots = await self.active_roots_in(view, ctx)
        for value in (
            await view.connection.execute(
                select(revisions.c.record)
                .where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.kind == "memory",
                    revisions.c.head.is_(True),
                )
                .order_by(revisions.c.commit_seq.desc())
            )
        ).scalars():
            record = CognitiveRecord.model_validate(value)
            if record.deleted and await self.visible_in(view, ctx, record, roots):
                result.append(record)
            if len(result) >= limit:
                break
        return tuple(result)


async def decay_memories(
    self: CognitionService, ctx: CallContext, operation_id: str, limit: int = 1000
):
    if not 1 <= limit <= 1000:
        raise DomainError("invalid_limit", "Decay limit must be between 1 and 1000")
    from .lifecycle import transition

    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="memory.decay",
        scope=ctx.require_subject(),
        payload={"limit": limit},
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        await require_subject_in(tx, ctx, "memory")
        now = int(__import__("datetime").datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        rows = (
            (
                await tx.connection.execute(
                    select(revisions.c.record)
                    .where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.subject_id == ctx.subject_id,
                        revisions.c.kind == "memory",
                        revisions.c.head.is_(True),
                    )
                    .order_by(revisions.c.commit_seq)
                    .limit(limit)
                )
            )
            .scalars()
            .all()
        )
        changed: list[str] = []
        for value in rows:
            record = CognitiveRecord.model_validate(value)
            if record.deleted or not await self.visible_in(tx, ctx, record):
                continue
            command = MemoryChange(
                reference=record.reference,
                action="decay",
                reason="scheduled memory retention evaluation",
            )
            lifecycle = record.lifecycle or MemoryLifecycle(
                anchor_unix_ms=record.times.recorded.coordinate
            )
            if transition(lifecycle, command, now) != lifecycle:
                changed.append((await change_memory_in(self, tx, ctx, command)).reference.exact())
        return tx.result({"scanned": len(rows), "changed": changed, "complete": len(rows) < limit})


async def change_memories_in(
    self: CognitionService, tx: WriteTx, ctx: CallContext, command: MemoryChanges
) -> tuple[CognitiveRecord, ...]:
    if len({reference.id for reference in command.references}) != len(command.references):
        raise DomainError("memory_selection", "A batch can select each Memory identity once")
    for reference in command.references:
        record = await self.get_in(tx, ctx, reference)
        current = await tx.connection.scalar(
            select(revisions.c.revision).where(
                revisions.c.id == reference.id,
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.subject_id == ctx.subject_id,
                revisions.c.head.is_(True),
            )
        )
        if reference.kind != "memory" or current != reference.revision or record.purged:
            raise DomainError(
                "memory_revision_conflict",
                "A frozen batch target must still be its current nonpurged Memory revision",
            )
    changed: list[CognitiveRecord] = []
    for reference in command.references:
        current = await self.get_in(tx, ctx, reference)
        if command.action == "purge" and current.purged:
            changed.append(current)
            continue
        changed.append(
            await change_memory_in(
                self,
                tx,
                ctx,
                MemoryChange(
                    operation_id=command.operation_id,
                    reference=reference,
                    action=command.action,
                    strength=command.strength,
                    hours=command.hours,
                    reason=command.reason,
                    force=command.force,
                ),
            )
        )
    return tuple(changed)


async def change_memories(
    self: CognitionService, ctx: CallContext, command: MemoryChanges
) -> tuple[CognitiveRecord, ...]:
    """Execute a frozen selection once; no repeated query can change its membership."""
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="memory.change_many",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await self.replay_records_in(
                tx, ctx, TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
            )
        result = tx.result(await self.change_memories_in(tx, ctx, command))
    if self.invalidate:
        await self.invalidate()
    return result
