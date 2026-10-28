"""Context session state owned behavior."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import (
    CallContext,
    DomainError,
)
from nous_wave.contracts.context import (
    ContextPreparation,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from .models import preparations, sessions

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def open_session(
    self: ManagedContexts, ctx: CallContext, operation_id: str, session_id: str | None = None
):
    from nous_wave.contracts.context import ContextSession

    identity = session_id or str(uuid4())
    key = sha256(
        f"{ctx.caller_id}\0{ctx.namespace_id}\0{ctx.require_subject()}\0{identity}".encode()
    ).hexdigest()
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="context.open_session",
        scope=ctx.require_subject(),
        payload={"session_id": session_id},
    ) as tx:
        if tx.replayed:
            return ContextSession.model_validate(tx.replay_result)
        existing = (
            await tx.connection.execute(select(sessions.c.record).where(sessions.c.id == key))
        ).scalar_one_or_none()
        if existing is not None:
            return tx.result(ContextSession.model_validate(existing))
        result = ContextSession(
            id=identity,
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            closed=False,
            recorded_at=tx.recorded_at,
            commit_seq=tx.commit_seq,
        )
        await tx.connection.execute(
            sessions.insert().values(
                id=key,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.subject_id,
                record=result.model_dump(mode="json"),
            )
        )
        return tx.result(result)


async def close_session(
    self: ManagedContexts, ctx: CallContext, session_id: str, operation_id: str
):
    from nous_wave.contracts.context import ContextSession

    key = sha256(
        f"{ctx.caller_id}\0{ctx.namespace_id}\0{ctx.require_subject()}\0{session_id}".encode()
    ).hexdigest()
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="context.close_session",
        scope=ctx.require_subject(),
        payload={"session_id": session_id},
    ) as tx:
        if tx.replayed:
            return ContextSession.model_validate(tx.replay_result)
        raw = (
            await tx.connection.execute(select(sessions.c.record).where(sessions.c.id == key))
        ).scalar_one_or_none()
        if raw is None:
            raise DomainError("session_not_found", "Context session is unavailable")
        result = ContextSession.model_validate(raw).model_copy(
            update={"closed": True, "closed_at": tx.recorded_at, "commit_seq": tx.commit_seq}
        )
        await tx.connection.execute(
            update(sessions)
            .where(sessions.c.id == key)
            .values(record=result.model_dump(mode="json"))
        )
        rows = (
            await tx.connection.execute(
                select(preparations.c.id, preparations.c.record).where(
                    preparations.c.namespace_id == ctx.namespace_id,
                    preparations.c.subject_id == ctx.subject_id,
                )
            )
        ).all()
        for row in rows:
            value = TypeAdapter(dict[str, JsonValue]).validate_python(row.record)
            if "preparation" not in value:
                continue
            preparation = ContextPreparation.model_validate(value["preparation"])
            if (
                value.get("caller_id") == ctx.caller_id
                and preparation.request.session_id == session_id
                and preparation.state == "prepared"
            ):
                value["preparation"] = TypeAdapter(JsonValue).validate_json(
                    preparation.model_copy(update={"state": "cancelled"}).model_dump_json()
                )
                await tx.connection.execute(
                    update(preparations).where(preparations.c.id == row.id).values(record=value)
                )
        return tx.result(result)
