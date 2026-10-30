"""Context session state owned behavior."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    CallContext,
    DomainError,
)
from nous_sdk.contracts.context import (
    ChangeContextSession,
    ContextPreparation,
    ContextSession,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.platform.persistence import WriteTx

from .models import preparations, sessions

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def open_session(
    self: ManagedContexts,
    ctx: CallContext,
    operation_id: str,
    session_id: str | None = None,
    *,
    parent_session_id: str | None = None,
    history_generation: str | None = None,
):

    identity = session_id or str(uuid4())
    key = sha256(
        f"{ctx.caller_id}\0{ctx.namespace_id}\0{ctx.require_subject()}\0{identity}".encode()
    ).hexdigest()
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="context.open_session",
        scope=ctx.require_subject(),
        payload={
            "session_id": session_id,
            "parent_session_id": parent_session_id,
            "history_generation": history_generation,
        },
    ) as tx:
        if tx.replayed:
            return await self.require_session_sources_in(
                tx, ctx, ContextSession.model_validate(tx.replay_result)
            )
        existing = (
            await tx.connection.execute(select(sessions.c.record).where(sessions.c.id == key))
        ).scalar_one_or_none()
        if existing is not None:
            return tx.result(
                await self.require_session_sources_in(
                    tx, ctx, ContextSession.model_validate(existing)
                )
            )
        if parent_session_id is not None:
            await self.session_in(tx, ctx, parent_session_id)
        result = ContextSession(
            id=identity,
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            closed=False,
            recorded_at=tx.recorded_at,
            commit_seq=tx.commit_seq,
            parent_session_id=parent_session_id,
            history_generation=history_generation,
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
    from nous_sdk.contracts.context import ContextSession

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
            return await self.require_session_sources_in(
                tx, ctx, ContextSession.model_validate(tx.replay_result)
            )
        raw = (
            await tx.connection.execute(select(sessions.c.record).where(sessions.c.id == key))
        ).scalar_one_or_none()
        if raw is None:
            raise DomainError("session_not_found", "Context session is unavailable")
        await self.require_session_sources_in(tx, ctx, ContextSession.model_validate(raw))
        result = ContextSession.model_validate(raw).model_copy(
            update={"closed": True, "closed_at": tx.recorded_at, "commit_seq": tx.commit_seq}
        )
        await tx.connection.execute(
            update(sessions)
            .where(sessions.c.id == key)
            .values(record=result.model_dump(mode="json"))
        )
        await cancel_pending_in(self, tx, ctx, session_id)
        return tx.result(result)


async def cancel_pending_in(
    self: ManagedContexts, tx: WriteTx, ctx: CallContext, session_id: str
) -> None:
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
            empty = preparation.response.projection.model_copy(update={"segments": ()})
            value["preparation"] = TypeAdapter(JsonValue).validate_json(
                preparation.model_copy(
                    update={
                        "state": "cancelled",
                        "response": preparation.response.model_copy(update={"projection": empty}),
                    }
                ).model_dump_json()
            )
            value["full_projection"] = TypeAdapter(JsonValue).validate_json(empty.model_dump_json())
            await tx.connection.execute(
                update(preparations).where(preparations.c.id == row.id).values(record=value)
            )


async def change_session(
    self: ManagedContexts, ctx: CallContext, command: ChangeContextSession
) -> ContextSession:
    from .lifecycle import session_key

    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="context.change_session",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await self.require_session_sources_in(
                tx, ctx, ContextSession.model_validate(tx.replay_result)
            )
        current = await self.session_in(tx, ctx, command.session_id)
        await self.require_session_sources_in(tx, ctx, current)
        if current.closed or current.revision != command.expected_revision:
            raise DomainError(
                "session_revision_conflict", "Session is closed or its revision changed"
            )
        updates: dict[str, object] = {"revision": current.revision + 1, "commit_seq": tx.commit_seq}
        if "active_work_context_id" in command.model_fields_set:
            if command.active_work_context_id is not None:
                work = await self.work_in(tx, ctx, command.active_work_context_id)
                if work.state != "open":
                    raise DomainError(
                        "work_context_inactive", "Only open work can enter an active session"
                    )
            updates["active_work_context_id"] = command.active_work_context_id
        if "history_generation" in command.model_fields_set:
            updates["history_generation"] = command.history_generation
        if command.invalidate_history or (
            "history_generation" in command.model_fields_set
            and command.history_generation != current.history_generation
        ):
            updates["history_invalidated"] = True
            await cancel_pending_in(self, tx, ctx, current.id)
        if command.close:
            updates.update({"closed": True, "closed_at": tx.recorded_at})
            await cancel_pending_in(self, tx, ctx, current.id)
        result = current.model_copy(update=updates)
        await tx.connection.execute(
            update(sessions)
            .where(sessions.c.id == session_key(ctx, current.id))
            .values(record=result.model_dump(mode="json"))
        )
        return tx.result(result)
