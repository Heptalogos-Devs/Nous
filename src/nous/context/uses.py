"""Record actual context uses through the same authorized event owner."""

from __future__ import annotations

from datetime import datetime
from hashlib import sha256
from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.context import ContextAdmission, ContextUse
from sqlalchemy import select, update

from nous.platform.persistence import WriteTx

from .models import feedback, sessions

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def record_uses_in(
    self: ManagedContexts,
    tx: WriteTx,
    ctx: CallContext,
    admission: ContextAdmission,
    uses: tuple[ContextUse, ...],
    occurred_at: datetime,
) -> None:
    for use in uses:
        if use.target not in admission.sources:
            raise DomainError(
                "context_use_not_admitted",
                "Reported use was not present in the admitted model input",
            )
        await self.require_targets_in(tx, ctx, (use.target,))
        event_key = sha256(f"{ctx.caller_id}\0{admission.id}\0{use.event_id}".encode()).hexdigest()
        event = {
            "id": event_key,
            "event_id": use.event_id,
            "context_id": admission.id,
            "target": use.target.model_dump(mode="json"),
            "kind": "actual_use",
            "use_kind": use.kind,
            "invocation_id": admission.invocation_id,
            "occurred_at": occurred_at.isoformat(),
            "commit_seq": tx.commit_seq,
        }
        existing_event = await tx.connection.scalar(
            select(feedback.c.record).where(feedback.c.id == event_key)
        )
        if existing_event is None:
            await tx.connection.execute(
                feedback.insert().values(id=event_key, context_id=admission.id, record=event)
            )
        elif (
            existing_event.get("target") != event["target"]
            or existing_event.get("use_kind") != use.kind
        ):
            raise DomainError(
                "context_use_conflict",
                "Use event identity already names a different observation",
            )
    if uses:
        from .lifecycle import session_key

        current_session = await self.session_in(tx, ctx, admission.session_id)
        updated_residents = tuple(
            item.model_copy(
                update={
                    "last_meaningful_use_at": max(
                        occurred_at, item.last_meaningful_use_at or item.entered_at
                    )
                }
            )
            if any(use.target == item.target for use in uses)
            else item
            for item in current_session.residents
        )
        revised_session = current_session.model_copy(
            update={
                "residents": updated_residents,
                "revision": current_session.revision + 1,
                "commit_seq": tx.commit_seq,
            }
        )
        await tx.connection.execute(
            update(sessions)
            .where(sessions.c.id == session_key(ctx, admission.session_id))
            .values(record=revised_session.model_dump(mode="json"))
        )
