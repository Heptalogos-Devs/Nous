"""Memory operations share the cognitive authority transaction."""

from __future__ import annotations

from typing import TYPE_CHECKING
from datetime import datetime
from nous_sdk.contracts import CognitiveRef, RecordsQuery, RetentionChanges, RetentionExpectation
from nous_sdk.contracts.retention import DecayRetention, RetentionDecayResult
from .lifecycle import snapshot, state_for, transition

from nous_sdk.contracts.lifecycle import MemoryChanges
from nous_sdk.contracts.public import RememberInput
from nous_sdk.contracts.common import utc_now
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.content import ContentStore
from nous.material.service import MaterialService

from .revision_state import RevisionState
from .schema import revisions, vectors

if TYPE_CHECKING:
    from .records import CognitiveRecords
from nous_sdk.contracts import (
    AdoptCognition,
    CognitiveRecord,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    MemoryChange,
    RetentionChange,
    RetentionState,
    RevisionCause,
    TimePoint,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError


class MemoryService:
    def __init__(
        self,
        authority: Authority,
        records: CognitiveRecords,
        *,
        materials: MaterialService,
        content: ContentStore,
    ):
        self.authority = authority
        self.records = records
        self.materials = materials
        self.content = content

    async def remember(self, ctx: CallContext, command: RememberInput) -> CognitiveRecord:
        document = ImportDocument(
            text=command.content,
            filename="memory.txt",
            source=command.source,
            source_type="explicit_memory",
            origin={"caller_id": ctx.caller_id},
        )
        store = self.content
        lease = (
            await store.publish_bytes(command.content.encode(), document.media_type)
            if store
            else None
        )
        try:
            async with self.authority.write(ctx) as tx:
                admission = await self.materials.admit_text_in(tx, ctx, document, lease)
                record = await self.records.adopt_in(
                    tx,
                    admission.context,
                    AdoptCognition(
                        body=MemoryBody(text=command.content, tags=command.tags),
                        evidence=(
                            EvidenceLink(
                                observation_id=admission.event["id"], role="explicit_memory"
                            ),
                        ),
                    ),
                    state=RevisionState(
                        cause=RevisionCause(
                            action="memory.remember", actor=ctx.caller_id, reason="Explicit memory"
                        ),
                        lifecycle=RetentionState(
                            anchor_unix_ms=utc_now().coordinate, pinned=command.pin
                        ),
                    ),
                )
                return record
        finally:
            if lease and store:
                await store.release(lease)

    async def change_memory_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        command: MemoryChange,
        *,
        revision_evidence: tuple[EvidenceLink, ...] = (),
    ) -> CognitiveRecord:
        if command.reference.kind != "memory":
            raise DomainError("memory_required", "Memory operations require a Memory reference")
        if command.action != "revise":
            return await self.change_retention_in(
                tx,
                ctx,
                RetentionChange(
                    reference=command.reference,
                    action=command.action,
                    strength=command.strength,
                    hours=command.hours,
                    reason=command.reason,
                    force=command.force,
                ),
            )
        previous = await self.records.get_in(tx, ctx, command.reference)
        if (
            previous.reference.kind != "memory"
            or previous.purged
            or (not isinstance(previous.body, MemoryBody))
        ):
            raise DomainError(
                "memory_unavailable", "Maintenance requires a nonpurged Memory revision"
            )
        if previous.deleted:
            raise DomainError("memory_deleted", "Restore the memory before changing it")
        if command.body is None:
            raise DomainError(
                "invalid_memory_edit", "Revise requires the complete edited memory body"
            )
        if not any(link.polarity == "support" for link in revision_evidence):
            raise DomainError(
                "memory_revision_source_required", "Admit the actual edited input first"
            )
        body = command.body or previous.body
        content_changed = (
            body.text != previous.body.text
            or body.summary != previous.body.summary
            or body.structured != previous.body.structured
            or (body.epistemic != previous.body.epistemic)
            or (body.role != previous.body.role)
            or (body.formation_mode != previous.body.formation_mode)
        )
        if content_changed:
            body = body.model_copy(
                update={
                    "media": tuple(
                        reference
                        for reference in body.media
                        if reference not in previous.body.media
                    ),
                    "contact_observation_id": None
                    if body.contact_observation_id == previous.body.contact_observation_id
                    else body.contact_observation_id,
                }
            )
        times = previous.times
        if content_changed:
            from nous.material.service import require_observation_in

            observed: list[TimePoint | None] = []
            for link in revision_evidence:
                if link.polarity != "support":
                    continue
                if link.reference is not None:
                    observed.append(
                        (await self.records.get_in(tx, ctx, link.reference)).times.observation
                    )
                elif link.observation_id is not None:
                    source = await require_observation_in(
                        tx, ctx, link.observation_id, region_id=link.region_id
                    )
                    observed.append(TimePoint.model_validate(source["record"]["observed_at"]))
            known = [point for point in observed if point is not None]
            observed_at = (
                max(known, key=lambda point: point.coordinate)
                if known
                and len(known) == len(observed)
                and (len({point.domain for point in known}) == 1)
                else None
            )
            times = times.model_copy(update={"formation": ctx.clock, "observation": observed_at})
        edit_evidence = revision_evidence
        if not content_changed:
            edit_evidence = (
                *previous.evidence,
                *(
                    link
                    if link.reference is not None
                    else link.model_copy(update={"polarity": "boundary"})
                    for link in revision_evidence
                ),
            )
        lifecycle = previous.lifecycle or RetentionState(
            anchor_unix_ms=previous.times.recorded.coordinate
        )
        state = RevisionState(
            lifecycle=lifecycle,
            cause=RevisionCause(
                action=command.action,
                actor=ctx.caller_id,
                reason=command.reason,
                parent=previous.reference,
            ),
        )
        result = await self.records.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=command.reference.revision,
                body=body,
                evidence=(
                    *edit_evidence,
                    EvidenceLink(
                        reference=previous.reference, polarity="boundary", role="previous content"
                    ),
                ),
                times=times,
            ),
            state=state,
        )
        return result

    async def revise_source_times_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        previous: CognitiveRecord,
        *,
        event: TimePoint | None = None,
        observation: TimePoint | None = None,
    ) -> CognitiveRecord:
        await self.records.get_in(tx, ctx, previous.reference)
        if not isinstance(previous.body, MemoryBody):
            raise DomainError("memory_required", "Source chronology applies to Memory")
        changes: dict[str, TimePoint] = {}
        if previous.times.event is None and event is not None:
            changes["event"] = event
        if previous.times.observation is None and observation is not None:
            changes["observation"] = observation
        if not changes:
            return previous
        result = await self.records.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=previous.body,
                evidence=(
                    *previous.evidence,
                    EvidenceLink(
                        reference=previous.reference, polarity="boundary", role="source chronology"
                    ),
                ),
                times=previous.times.model_copy(update=changes),
                producer=previous.producer,
            ),
            state=RevisionState(
                cause=RevisionCause(
                    action="memory.source_times",
                    actor=ctx.caller_id,
                    reason="Recovered explicit source time fields",
                    parent=previous.reference,
                ),
                lifecycle=previous.lifecycle,
                deleted=previous.deleted,
            ),
        )
        assets = (
            (
                await tx.connection.execute(
                    select(vectors).where(
                        vectors.c.namespace_id == ctx.namespace_id,
                        vectors.c.reference == previous.reference.exact(),
                    )
                )
            )
            .mappings()
            .all()
        )
        for asset in assets:
            await tx.connection.execute(
                vectors.insert().values(
                    **{**asset, "reference": result.reference.exact(), "commit_seq": tx.commit_seq}
                )
            )
        return result

    async def change_memory(self, ctx: CallContext, command: MemoryChange) -> CognitiveRecord:
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
        store = self.content
        lease = (
            await store.publish_bytes(document.inline_text().encode(), document.media_type)
            if document is not None and store
            else None
        )
        try:
            async with self.authority.write(ctx) as tx:
                effective, revision_evidence = (ctx, ())
                if document is not None:
                    await self.records.get_in(tx, ctx, command.reference)
                    admission = await self.materials.admit_text_in(tx, ctx, document, lease)
                    effective = admission.context
                    revision_evidence = (
                        EvidenceLink(observation_id=admission.event["id"], role="memory_revision"),
                    )
                result = await self.change_memory_in(tx, effective, command, revision_evidence=revision_evidence)
        finally:
            if lease and store:
                await store.release(lease)
        return result

    async def recycle_bin(self, ctx: CallContext, limit: int = 50) -> tuple[CognitiveRecord, ...]:
        if not 1 <= limit <= 1000:
            raise DomainError("invalid_limit", "Recycle-bin limit must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "memory")
            result: list[CognitiveRecord] = []
            roots = await self.records.source_roots_in(view, ctx)
            for value in (
                await view.connection.execute(
                    select(revisions.c.record)
                    .where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.kind == "memory",
                        revisions.c.head.is_(True),
                    )
                    .order_by(revisions.c.commit_seq.desc())
                )
            ).scalars():
                record = CognitiveRecord.model_validate(value)
                if record.deleted and await self.records.visible_in(view, ctx, record, roots):
                    result.append(record)
                if len(result) >= limit:
                    break
            return tuple(result)

    async def decay_memories(self, ctx: CallContext, operation_id: str, limit: int = 1000):
        from nous_sdk.contracts.retention import DecayRetention

        async with self.authority.write(ctx) as tx:
            await require_namespace_in(tx, ctx, "memory")
            result = await self.decay_retention_in(
                tx, ctx, DecayRetention(kinds=("memory",), limit=limit)
            )
            return {'scanned': result.scanned, 'changed': [ref.exact() for ref in result.changed], 'complete': result.complete, 'next_cursor': result.next_cursor}

    async def change_memories_in(
        self, tx: WriteTx, ctx: CallContext, command: MemoryChanges
    ) -> tuple[CognitiveRecord, ...]:
        from nous_sdk.contracts import RetentionChanges

        if any(reference.kind != "memory" for reference in command.references):
            raise DomainError("memory_required", "Memory operations require Memory references")
        return await self.change_retentions_in(
            tx, ctx, RetentionChanges.model_validate(command.model_dump())
        )

    async def change_memories(
        self, ctx: CallContext, command: MemoryChanges
    ) -> tuple[CognitiveRecord, ...]:
        """Execute a frozen selection once; no repeated query can change its membership."""
        async with self.authority.write(ctx) as tx:
            result = await self.change_memories_in(tx, ctx, command)
        return result

    async def require_retention_preconditions_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        expectations: tuple[RetentionExpectation, ...],
    ) -> None:
        if not expectations:
            return
        await require_namespace_in(view, ctx)
        for expected in expectations:
            raw = await view.connection.scalar(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.kind == expected.reference.kind,
                    revisions.c.id == expected.reference.id,
                    revisions.c.head.is_(True),
                )
            )
            if raw is None:
                raise DomainError(
                    "retention_precondition_failed",
                    "The expected retained object is no longer present",
                    reference=expected.reference.exact(),
                )
            record = CognitiveRecord.model_validate(raw)
            await self.records.get_in(view, ctx, record.reference, purpose="management")
            if record.purged or record.reference.kind not in {"memory", "association"}:
                raise DomainError(
                    "retention_precondition_failed",
                    "The expected retained object is unavailable",
                    reference=expected.reference.exact(),
                )
            actual = snapshot(
                record,
                int(datetime.fromisoformat(view.recorded_at).timestamp() * 1000)
                if isinstance(view, WriteTx)
                else utc_now().coordinate,
            )
            actual_fields = actual.model_dump(mode="json", exclude={"score"})
            wanted = expected.model_dump(mode="json")
            if actual_fields != wanted:
                raise DomainError(
                    "retention_precondition_failed",
                    "Retention state changed after selection",
                    reference=expected.reference.exact(),
                    current=record.reference.exact(),
                    fields=[name for name in wanted if actual_fields.get(name) != wanted[name]],
                )

    async def change_retention_in(
        self, tx: WriteTx, ctx: CallContext, command: RetentionChange
    ) -> CognitiveRecord:
        previous = await self.records.get_in(tx, ctx, command.reference)
        lifecycle = state_for(previous)
        current = await tx.connection.scalar(
            select(revisions.c.revision).where(
                revisions.c.id == previous.reference.id,
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.head.is_(True),
            )
        )
        if current != command.reference.revision:
            raise DomainError(
                "retention_revision_conflict",
                "The retained object changed after selection",
                reference=command.reference.exact(),
            )
        if previous.deleted and command.action not in {"restore", "purge"}:
            raise DomainError("retention_deleted", "Restore the retained object before changing it")
        if command.hours is not None and command.action != "protect":
            raise DomainError("invalid_retention_hours", "Hours applies to protection")
        now = int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        if (
            command.action in {"trash", "purge"}
            and lifecycle.protected(now)
            and (not command.force)
        ):
            raise DomainError(
                "retention_protected",
                "The retained object is protected; unprotect it or explicitly force the change",
            )
        updated = transition(lifecycle, command, now)
        deleted = (
            command.action in {"trash", "purge"}
            if command.action in {"trash", "purge", "restore"}
            else previous.deleted
        )
        if updated == lifecycle and deleted == previous.deleted and (command.action != "purge"):
            return previous
        assert previous.body is not None
        result = await self.records.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=previous.body,
                evidence=(
                    EvidenceLink(
                        reference=previous.reference, role=command.reason or command.action
                    ),
                ),
                times=previous.times,
                producer=previous.producer,
            ),
            state=RevisionState(
                lifecycle=updated,
                deleted=deleted,
                purged=command.action == "purge",
                cause=RevisionCause(
                    action=command.action,
                    actor=ctx.caller_id,
                    reason=command.reason,
                    parent=previous.reference,
                ),
            ),
        )
        if command.action == "purge":
            await self.records.erase_history_in(tx, ctx, previous.reference.id)
        else:
            for asset in (
                await tx.connection.execute(
                    select(vectors).where(
                        vectors.c.namespace_id == ctx.namespace_id,
                        vectors.c.reference == previous.reference.exact(),
                    )
                )
            ).mappings():
                await tx.connection.execute(
                    vectors.insert().values(
                        **{
                            **asset,
                            "reference": result.reference.exact(),
                            "commit_seq": tx.commit_seq,
                        }
                    )
                )
        return result

    async def change_retentions_in(
        self, tx: WriteTx, ctx: CallContext, command: RetentionChanges
    ) -> tuple[CognitiveRecord, ...]:
        if len({ref.id for ref in command.references}) != len(command.references):
            raise DomainError("retention_selection", "Select each retained identity once")
        for reference in command.references:
            record = await self.records.get_in(tx, ctx, reference)
            state_for(record)
            current = await tx.connection.scalar(
                select(revisions.c.revision).where(
                    revisions.c.id == reference.id,
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.head.is_(True),
                )
            )
            if current != reference.revision:
                raise DomainError(
                    "retention_revision_conflict",
                    "The frozen lifecycle selection changed",
                    reference=reference.exact(),
                )
        return tuple(
            [
                await self.change_retention_in(
                    tx,
                    ctx,
                    RetentionChange(
                        reference=reference,
                        action=command.action,
                        strength=command.strength,
                        hours=command.hours,
                        reason=command.reason,
                        force=command.force,
                    ),
                )
                for reference in command.references
            ]
        )

    async def change_retention(self, ctx: CallContext, command: RetentionChange) -> CognitiveRecord:
        async with self.authority.write(ctx) as tx:
            result = await self.change_retention_in(tx, ctx, command)
        return result

    async def change_retentions(
        self, ctx: CallContext, command: RetentionChanges
    ) -> tuple[CognitiveRecord, ...]:

        async with self.authority.write(ctx) as tx:
            result = await self.change_retentions_in(tx, ctx, command)
        return result

    async def retention_snapshot(self, ctx: CallContext, reference: CognitiveRef):
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            return snapshot(
                await self.records.get_in(view, ctx, reference, purpose="management"),
                utc_now().coordinate,
            )

    async def decay_retention_in(
        self, tx: WriteTx, ctx: CallContext, command: DecayRetention
    ) -> RetentionDecayResult:
        page = await self.records.records_in(
            tx, ctx, RecordsQuery(kinds=command.kinds, limit=command.limit, cursor=command.cursor)
        )
        now = int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        changed: list[CognitiveRef] = []
        for entry in page.items:
            before = state_for(entry.record)
            change = RetentionChange(
                reference=entry.record.reference, action="decay", reason="retention evaluation"
            )
            if transition(before, change, now) != before:
                changed.append((await self.change_retention_in(tx, ctx, change)).reference)
        return RetentionDecayResult(
            scanned=len(page.items),
            changed=tuple(changed),
            next_cursor=page.next_cursor,
            complete=page.next_cursor is None,
        )

    async def decay_retention(
        self, ctx: CallContext, command: DecayRetention
    ) -> RetentionDecayResult:
        async with self.authority.write(ctx) as tx:
            result = await self.decay_retention_in(tx, ctx, command)
        return result

    async def purge_retained_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        reference: CognitiveRef,
        *,
        force: bool = False,
        reason: str,
    ) -> CognitiveRecord:
        previous = await self.records.get_in(tx, ctx, reference, purpose="management")
        if previous.purged:
            return previous
        if await self.records.visible_in(tx, ctx, previous):
            return await self.change_retention_in(
                tx,
                ctx,
                RetentionChange(reference=reference, action="purge", force=force, reason=reason),
            )
        lifecycle = state_for(previous)
        now = round(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        if lifecycle.protected(now) and not force:
            raise DomainError(
                "retention_protected",
                "The retained object is protected; explicitly force its removal",
            )
        await self.records.erase_history_in(tx, ctx, reference.id)
        return previous.model_copy(update={"body": None, "purged": True, "deleted": True})
