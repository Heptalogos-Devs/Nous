"""Automatic residency respects current activation while retaining exact source identity."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal
from pydantic import Field
from nous_sdk.contracts import Contract, QueryRequest
from nous_sdk.contracts.context import ContextTarget, ContextUsage, ContextCursor
from nous_sdk.contracts.history import WorkingHistoryGroup

from nous_sdk.contracts import CognitiveRecord, CognitiveRef
from nous.platform.call_context import CallContext
from nous_sdk.contracts.context import CognitiveTarget, ContextSegment

from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .runtime import ContextRuntime


class SelectionRequest(Contract):
    session_id: str
    consumer_id: str
    instance_id: str
    usage: ContextUsage
    trigger: Literal[
        "ordinary", "task_change", "tool_result", "conflict", "resume", "pause", "budget"
    ] = "ordinary"
    present_sources: tuple[ContextTarget, ...] = ()
    working_sources: tuple[ContextTarget, ...] = ()
    working_groups: tuple[WorkingHistoryGroup, ...] = ()
    history_source_bytes: int = Field(default=0, ge=0)
    include_optional: bool = True
    max_items: int = Field(default=64, ge=1, le=256)
    max_text_bytes: int = Field(default=65536, ge=1, le=1048576)
    current_sources: tuple[ContextTarget, ...] = ()
    query: QueryRequest | None = None
    known_cursor: ContextCursor | None = None


async def automatic_resident_in(
    self: ContextRuntime,
    view: ReadView | WriteTx,
    ctx: CallContext,
    reference: CognitiveRef,
) -> CognitiveRecord | None:
    return await self.cognition.automatic_in(view, ctx, reference)


async def segment_eligible_in(
    self: ContextRuntime,
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
