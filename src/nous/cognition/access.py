"""Public history and identity resolution use the same source visibility as reads."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    ConceptBody,
    DomainError,
    EvidenceLink,
    MemoryBody,
    MemoryChange,
)
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.platform.persistence import ReadView, WriteTx

from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from .service import CognitionService


async def history(
    self: CognitionService, ctx: CallContext, object_id: str, limit: int = 100
) -> tuple[CognitiveRecord, ...]:
    if not 1 <= limit <= 1000:
        raise DomainError("invalid_limit", "History limit must be between 1 and 1000")
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        values = (
            await view.connection.execute(
                select(revisions.c.record)
                .where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.id == object_id,
                )
                .order_by(revisions.c.revision.desc())
                .limit(limit)
            )
        ).scalars()
        result: list[CognitiveRecord] = []
        for value in values:
            record = CognitiveRecord.model_validate(value)
            tombstone_visible = False
            if record.purged:
                from nous.material.service import observations

                past_roots = {
                    str(row.id): str(row.material_id)
                    for row in (
                        await view.connection.execute(
                            select(observations.c.id, observations.c.material_id).where(
                                observations.c.namespace_id == ctx.namespace_id,
                                observations.c.subject_id == ctx.subject_id,
                            )
                        )
                    ).all()
                }
                tombstone_visible = await self.visible_in(view, ctx, record, past_roots)
            if tombstone_visible or await self.visible_in(view, ctx, record):
                result.append(record)
        return tuple(result)


async def concept_by_alias(self: CognitionService, ctx: CallContext, label: str) -> CognitiveRecord:
    key = " ".join(label.casefold().split())
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        values = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.kind == "concept",
                    revisions.c.head.is_(True),
                )
            )
        ).scalars()
        matches: list[CognitiveRecord] = []
        for value in values:
            record = CognitiveRecord.model_validate(value)
            if (
                isinstance(record.body, ConceptBody)
                and not record.deleted
                and record.body.state == "active"
                and key
                in {
                    " ".join(term.casefold().split())
                    for term in (record.body.label, *record.body.aliases)
                }
                and await self.visible_in(view, ctx, record)
            ):
                matches.append(record)
        if not matches:
            raise DomainError("concept_not_found", "No accessible Concept matches this alias")
        if len(matches) > 1:
            raise DomainError("ambiguous_name", "The alias identifies multiple accessible Concepts")
        return matches[0]


async def set_deleted(
    self: CognitionService,
    ctx: CallContext,
    reference: CognitiveRef,
    deleted: bool,
    operation_id: str,
) -> CognitiveRecord:
    async with self.authority.read(ctx) as view:
        previous = await self.get_in(view, ctx, reference)
    if isinstance(previous.body, MemoryBody):
        return await self.change_memory(
            ctx,
            MemoryChange(
                operation_id=operation_id,
                reference=reference,
                action="trash" if deleted else "restore",
            ),
        )
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="cognition.set_deleted",
        scope=ctx.require_subject(),
        payload={"reference": reference.model_dump(mode="json"), "deleted": deleted},
    ) as tx:
        if tx.replayed:
            return await self.replay_record_in(
                tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
            )
        adopted = await self.set_deleted_in(tx, ctx, reference, deleted)
        result = tx.result(adopted)
    if self.invalidate:
        await self.invalidate()
    return result


async def set_deleted_in(
    self: CognitionService,
    tx: WriteTx,
    ctx: CallContext,
    reference: CognitiveRef,
    deleted: bool,
    *,
    evidence: tuple[EvidenceLink, ...] = (),
    reason: str = "Manual cognitive lifecycle change",
) -> CognitiveRecord:
    previous = await self.get_in(tx, ctx, reference)
    if isinstance(previous.body, MemoryBody):
        return await self.change_memory_in(
            tx,
            ctx,
            MemoryChange(
                reference=reference, action="trash" if deleted else "restore", reason=reason
            ),
        )
    if previous.body is None:
        raise DomainError("cognition_purged", "A purged object cannot be changed")
    from nous_sdk.contracts import RevisionCause

    return await self.adopt_in(
        tx,
        ctx,
        AdoptCognition(
            id=reference.id,
            expected_revision=reference.revision,
            body=previous.body,
            evidence=(EvidenceLink(reference=reference), *evidence),
            times=previous.times,
        ),
        state=RevisionState(
            deleted=deleted,
            lifecycle=previous.lifecycle,
            cause=RevisionCause(
                action="trash" if deleted else "restore",
                actor=ctx.caller_id,
                reason=reason,
                parent=reference,
            ),
        ),
    )


async def replay_record_in(
    self: CognitionService, view: ReadView | WriteTx, ctx: CallContext, record: CognitiveRecord
) -> CognitiveRecord:
    """An operation receipt never preserves an older, broader source authorization."""
    if record.purged:
        return record
    return await self.get_in(view, ctx, record.reference)


async def replay_records_in(
    self: CognitionService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    records: tuple[CognitiveRecord, ...],
) -> tuple[CognitiveRecord, ...]:
    return tuple([await self.replay_record_in(view, ctx, record) for record in records])
