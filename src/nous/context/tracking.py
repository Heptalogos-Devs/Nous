"""Read persisted admitted tracks under the current source authorization."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.context import (
    ContextPreparation,
    ContextPrepare,
    ContextProjection,
    ContextTrack,
    ContextTrackState,
)
from sqlalchemy import select

from .models import preparations, tracks

if TYPE_CHECKING:
    from .managed import ManagedContexts


def track_identity(ctx: CallContext, command: ContextTrack | ContextPrepare) -> str:
    return sha256(
        f"{ctx.caller_id}\0{ctx.namespace_id}\0{ctx.require_subject()}\0{command.session_id}\0{command.consumer_id}\0{command.instance_id}".encode()
    ).hexdigest()


async def read_track(
    self: ManagedContexts, ctx: CallContext, command: ContextTrack
) -> ContextTrackState:
    async with self.authority.read(ctx) as view:
        raw = await view.connection.scalar(
            select(tracks.c.record).where(
                tracks.c.id == track_identity(ctx, command),
                tracks.c.namespace_id == ctx.namespace_id,
                tracks.c.subject_id == ctx.require_subject(),
            )
        )
        projection = ContextProjection.model_validate(raw) if raw is not None else None
        reset_reason = None
        if projection is not None:
            try:
                await self.require_targets_in(
                    view,
                    ctx,
                    tuple(
                        target
                        for segment in projection.segments
                        for target in (*segment.sources, *segment.dependencies)
                    ),
                )
            except DomainError as error:
                if error.code not in {
                    "source_unavailable",
                    "cognition_not_found",
                    "region_not_found",
                    "material_not_found",
                }:
                    raise
                projection, reset_reason = None, "source_unavailable"
        pending: list[str] = []
        values = (
            await view.connection.execute(
                select(preparations.c.record).where(
                    preparations.c.track_id == track_identity(ctx, command),
                    preparations.c.namespace_id == ctx.namespace_id,
                    preparations.c.subject_id == ctx.subject_id,
                )
            )
        ).scalars()
        for value in values:
            if value.get("caller_id") != ctx.caller_id:
                continue
            prepared = ContextPreparation.model_validate(value["preparation"])
            if prepared.state == "prepared":
                pending.append(prepared.id)
        return ContextTrackState(
            projection=projection, pending_preparation_ids=tuple(pending), reset_reason=reset_reason
        )
