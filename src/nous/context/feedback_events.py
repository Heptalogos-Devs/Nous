"""All context consumers record the same authorized contact/use/outcome facts."""

from __future__ import annotations
from hashlib import sha256
from typing import TYPE_CHECKING
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.context import ContextTarget
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select
from nous.platform.persistence import WriteTx
from .models import feedback

if TYPE_CHECKING:
    from .runtime import ContextRuntime


async def record_in(
    self: ContextRuntime,
    tx: WriteTx,
    ctx: CallContext,
    *,
    context_id: str,
    event_id: str,
    target: ContextTarget,
    kind: str,
    occurred_at: str,
    session_id: str | None = None,
    invocation_id: str | None = None,
    use_kind: str | None = None,
    description: str = "",
    outcome: str = "unknown",
    subject_view: bool = False,
) -> dict[str, JsonValue]:
    await self.require_targets_in(tx, ctx, (target,))
    key = sha256(f"{ctx.caller_id}\x00{context_id}\x00{event_id}".encode()).hexdigest()
    event = TypeAdapter(dict[str, JsonValue]).validate_python(
        {
            "id": key,
            "event_id": event_id,
            "context_id": context_id,
            "namespace_id": ctx.namespace_id,
            "caller_id": ctx.caller_id,
            "target": target.model_dump(mode="json"),
            "kind": kind,
            "use_kind": use_kind,
            "description": description,
            "outcome": outcome,
            "session_id": session_id,
            "invocation_id": invocation_id,
            "occurred_at": occurred_at,
            "subject_view": subject_view,
            "commit_seq": tx.commit_seq,
        }
    )
    existing = await tx.connection.scalar(select(feedback.c.record).where(feedback.c.id == key))
    if existing is not None:
        if any(
            (
                existing.get(name) != event[name]
                for name in (
                    "target",
                    "kind",
                    "use_kind",
                    "description",
                    "outcome",
                    "invocation_id",
                    "occurred_at",
                )
            )
        ):
            raise DomainError(
                "context_use_conflict", "The feedback event already names another observation"
            )
        return TypeAdapter(dict[str, JsonValue]).validate_python(existing)
    await tx.connection.execute(
        feedback.insert().values(
            id=key, context_id=context_id, namespace_id=ctx.namespace_id, record=event
        )
    )
    from nous.retrieval.accretion import meaningful

    if meaningful(event):
        from nous.retrieval.maintenance import enqueue_index_in

        await enqueue_index_in(tx, ctx)
    return event
