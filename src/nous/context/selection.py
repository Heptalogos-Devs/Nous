"""Automatic residency respects current activation while retaining exact source identity."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, CognitiveRecord, CognitiveRef
from nous_wave.contracts.context import CognitiveTarget, ContextSegment
from sqlalchemy import select

from nous.cognition.lifecycle import selectable
from nous.cognition.schema import revisions
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def automatic_resident_in(
    self: ManagedContexts,
    view: ReadView | WriteTx,
    ctx: CallContext,
    reference: CognitiveRef,
) -> CognitiveRecord | None:
    exact = await self.cognition.get_in(view, ctx, reference)
    if not selectable(exact):
        return None
    raw = await view.connection.scalar(
        select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.subject_id == ctx.require_subject(),
            revisions.c.id == reference.id,
            revisions.c.head.is_(True),
        )
    )
    if raw is None or not selectable(CognitiveRecord.model_validate(raw)):
        return None
    return exact


async def segment_eligible_in(
    self: ManagedContexts,
    view: ReadView | WriteTx,
    ctx: CallContext,
    segment: ContextSegment,
) -> bool:
    await self.require_targets_in(view, ctx, segment.sources)
    if not segment.required:
        for target in segment.sources:
            if (
                isinstance(target, CognitiveTarget)
                and await automatic_resident_in(self, view, ctx, target.reference) is None
            ):
                return False
    return True
