"""Session residency and active work reuse the context owner's authoritative records."""

from datetime import UTC, datetime, timedelta
from hashlib import sha256
from uuid import uuid4
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.context import (
    ChangeResidents,
    ChangeWorkContext,
    ContextResident,
    ContextSession,
    ContextsPolicy,
    ContextTarget,
    WorkContext,
)
from sqlalchemy import select, update
from nous.platform.configuration.service import ConfigurationService
from nous.platform.persistence import Authority, ReadView, WriteTx
from .models import works, sessions


def session_key(ctx: CallContext, identity: str) -> str:
    return sha256(f"{ctx.caller_id}\x00{ctx.namespace_id}\x00{identity}".encode()).hexdigest()


async def session_record_in(
    view: ReadView | WriteTx, ctx: CallContext, identity: str
) -> ContextSession:
    raw = await view.connection.scalar(
        select(sessions.c.record).where(
            sessions.c.id == session_key(ctx, identity), sessions.c.namespace_id == ctx.namespace_id
        )
    )
    if raw is None:
        raise DomainError("session_not_found", "Context session is unavailable")
    return ContextSession.model_validate(raw)


async def work_record_in(view: ReadView | WriteTx, ctx: CallContext, identity: str) -> WorkContext:
    raw = await view.connection.scalar(
        select(works.c.record).where(
            works.c.namespace_id == ctx.namespace_id,
            works.c.caller_id == ctx.caller_id,
            works.c.id == identity,
            works.c.head.is_(True),
        )
    )
    if raw is None:
        raise DomainError("work_context_not_found", "Work context is unavailable")
    return WorkContext.model_validate(raw)


class ContextLifecycle:
    authority: Authority
    configuration: ConfigurationService

    async def require_targets_in(
        self, view: ReadView | WriteTx, ctx: CallContext, targets: tuple[ContextTarget, ...]
    ) -> None:
        raise NotImplementedError

    async def session_in(
        self, view: ReadView | WriteTx, ctx: CallContext, identity: str
    ) -> ContextSession:
        return await session_record_in(view, ctx, identity)

    async def require_session_sources_in(
        self, view: ReadView | WriteTx, ctx: CallContext, value: ContextSession
    ) -> ContextSession:
        await self.require_targets_in(
            view, ctx, tuple((item.target for item in value.residents if item.state != "evicted"))
        )
        return value

    async def read_session(self, ctx: CallContext, identity: str) -> ContextSession:
        async with self.authority.read(ctx) as view:
            return await self.require_session_sources_in(
                view, ctx, await self.session_in(view, ctx, identity)
            )

    async def policy(self, ctx: CallContext) -> ContextsPolicy:
        return ContextsPolicy.model_validate(
            (await self.configuration.effective(ctx, "contexts"))["value"]
        )

    async def ensure_residency_schedule(self, ctx: CallContext, session_id: str) -> None:
        raise NotImplementedError

    async def change_residents(self, ctx: CallContext, command: ChangeResidents) -> ContextSession:
        result = await self._change_residents(ctx, command)
        await self.ensure_residency_schedule(ctx, command.session_id)
        return result

    async def _change_residents(self, ctx: CallContext, command: ChangeResidents) -> ContextSession:
        policy = await self.policy(ctx)
        async with self.authority.write(ctx) as tx:
            return await self._change_residents_in(tx, ctx, command, policy)

    async def _change_residents_in(
        self, tx: WriteTx, ctx: CallContext, command: ChangeResidents, policy: ContextsPolicy
    ) -> ContextSession:
        current = await self.session_in(tx, ctx, command.session_id)
        await self.require_session_sources_in(tx, ctx, current)
        if current.closed or current.revision != command.expected_revision:
            raise DomainError(
                "session_revision_conflict", "Session is closed or its revision changed"
            )
        residents = list(current.residents)
        if command.action == "clear":
            residents = [item.model_copy(update={"state": "evicted"}) for item in residents]
        elif command.action == "evict":
            residents = [
                item.model_copy(update={"state": "evicted"})
                if item.target in command.targets
                else item
                for item in residents
            ]
        else:
            await self.require_targets_in(
                tx, ctx, tuple((item.target for item in command.residents))
            )
            incoming = {
                item.target.model_dump_json(): ContextResident.model_validate(
                    {**item.model_dump(), "entered_at": tx.recorded_at}
                )
                for item in command.residents
            }
            residents = [
                item for item in residents if item.target.model_dump_json() not in incoming
            ]
            residents.extend(incoming.values())
        if sum((item.state != "evicted" for item in residents)) > policy.resident_limit:
            raise DomainError("resident_limit", "Session residency exceeds the configured limit")
        result = current.model_copy(
            update={
                "residents": tuple(residents),
                "revision": current.revision + 1,
                "commit_seq": tx.commit_seq,
            }
        )
        await tx.connection.execute(
            update(sessions)
            .where(sessions.c.id == session_key(ctx, current.id))
            .values(record=result.model_dump(mode="json"))
        )
        return result

    async def work_in(
        self, view: ReadView | WriteTx, ctx: CallContext, identity: str
    ) -> WorkContext:
        result = await work_record_in(view, ctx, identity)
        if not result.purged:
            await self.require_targets_in(view, ctx, result.content.anchors)
        return result

    async def read_work(self, ctx: CallContext, identity: str) -> WorkContext:
        async with self.authority.read(ctx) as view:
            return await self.work_in(view, ctx, identity)

    async def change_work(self, ctx: CallContext, command: ChangeWorkContext) -> WorkContext:
        async with self.authority.write(ctx) as tx:
            return await self.change_work_in(tx, ctx, command)

    async def change_work_in(
        self, tx: WriteTx, ctx: CallContext, command: ChangeWorkContext
    ) -> WorkContext:
        if command.action == "create":
            if command.work_context_id is not None or command.content is None:
                raise DomainError(
                    "work_context_invalid", "Creation requires content and a new identity"
                )
            await self.require_targets_in(tx, ctx, command.content.anchors)
            result = WorkContext(
                id=str(uuid4()),
                namespace_id=ctx.namespace_id,
                revision=1,
                content=command.content,
                recorded_at=tx.recorded_at,
                updated_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
            )
        else:
            if command.work_context_id is None:
                raise DomainError("work_context_required", "Select a work context")
            current = await self.work_in(tx, ctx, command.work_context_id)
            if command.expected_revision != current.revision:
                raise DomainError("work_revision_conflict", "Work context revision changed")
            if current.state == "ended":
                raise DomainError("work_context_ended", "Ended work context is immutable")
            updates: dict[str, object] = {
                "revision": current.revision + 1,
                "updated_at": tx.recorded_at,
                "commit_seq": tx.commit_seq,
            }
            if command.action == "edit":
                if command.content is None:
                    raise DomainError("work_content_required", "Editing requires complete content")
                await self.require_targets_in(tx, ctx, command.content.anchors)
                updates["content"] = command.content
                updates["purged"] = False
            elif command.action in {"pause", "resume", "end"}:
                if command.action == "resume" and current.purged:
                    raise DomainError(
                        "work_context_source_purged", "Edit the purged work content before resuming"
                    )
                updates["state"] = {"pause": "suspended", "resume": "open", "end": "ended"}[
                    command.action
                ]
            elif command.action == "clear_text":
                updates["content"] = current.content.model_copy(update={"context_text": ""})
            elif command.action == "clear_anchors":
                updates["content"] = current.content.model_copy(update={"anchors": ()})
            elif command.action in {"pin", "unpin"}:
                await self.require_targets_in(tx, ctx, command.anchors)
                anchors = list(current.content.anchors)
                if command.action == "pin":
                    anchors.extend((target for target in command.anchors if target not in anchors))
                else:
                    anchors = [target for target in anchors if target not in command.anchors]
                updates["content"] = current.content.model_copy(update={"anchors": tuple(anchors)})
            result = current.model_copy(update=updates)
        await tx.connection.execute(
            update(works)
            .where(
                works.c.namespace_id == ctx.namespace_id,
                works.c.caller_id == ctx.caller_id,
                works.c.id == result.id,
                works.c.head.is_(True),
            )
            .values(head=False)
        )
        await tx.connection.execute(
            works.insert().values(
                namespace_id=ctx.namespace_id,
                caller_id=ctx.caller_id,
                id=result.id,
                revision=result.revision,
                head=True,
                record=result.model_dump(mode="json"),
            )
        )
        if command.action == "end":
            active_sessions = (
                await tx.connection.execute(
                    select(sessions.c.id, sessions.c.record).where(
                        sessions.c.namespace_id == ctx.namespace_id
                    )
                )
            ).all()
            for row in active_sessions:
                session = ContextSession.model_validate(row.record)
                if session.active_work_context_id != result.id or row.id != session_key(
                    ctx, session.id
                ):
                    continue
                revised = session.model_copy(
                    update={
                        "active_work_context_id": None,
                        "revision": session.revision + 1,
                        "commit_seq": tx.commit_seq,
                    }
                )
                await tx.connection.execute(
                    update(sessions)
                    .where(sessions.c.id == row.id)
                    .values(record=revised.model_dump(mode="json"))
                )
        return result

    async def work_history(
        self, ctx: CallContext, identity: str, limit: int = 100
    ) -> tuple[WorkContext, ...]:
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.scalars(
                    select(works.c.record)
                    .where(
                        works.c.namespace_id == ctx.namespace_id,
                        works.c.caller_id == ctx.caller_id,
                        works.c.id == identity,
                    )
                    .order_by(works.c.revision.desc())
                    .limit(limit)
                )
            ).all()
            values = tuple(WorkContext.model_validate(row) for row in rows)
            for value in values:
                await self.require_targets_in(view, ctx, value.content.anchors)
            return values

    async def list_work(
        self, ctx: CallContext, *, state: str | None = None, limit: int = 100
    ) -> tuple[WorkContext, ...]:
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.scalars(
                    select(works.c.record).where(
                        works.c.namespace_id == ctx.namespace_id,
                        works.c.caller_id == ctx.caller_id,
                        works.c.head.is_(True),
                    )
                )
            ).all()
            result = []
            for row in rows:
                value = WorkContext.model_validate(row)
                if state is not None and value.state != state:
                    continue
                await self.require_targets_in(view, ctx, value.content.anchors)
                result.append(value)
            return tuple(result[:limit])

    async def maintain_session(
        self, ctx: CallContext, identity: str, operation_id: str
    ) -> ContextSession:
        """Expire provisional/idle residents; contact alone never renews meaningful use."""
        policy = await self.policy(ctx)
        now = datetime.now(UTC)
        async with self.authority.write(ctx) as tx:
            current = await self.session_in(tx, ctx, identity)
            revised: list[ContextResident] = []
            for resident in current.residents:
                basis = resident.last_meaningful_use_at or resident.entered_at
                due = basis + timedelta(seconds=policy.resident_idle_seconds) <= now
                held = resident.hold_until is not None and resident.hold_until > now
                revised.append(
                    resident.model_copy(update={"state": "evicted"})
                    if due and (not held)
                    else resident
                )
            result = current.model_copy(
                update={
                    "residents": tuple(revised),
                    "revision": current.revision + 1,
                    "commit_seq": tx.commit_seq,
                }
            )
            await tx.connection.execute(
                update(sessions)
                .where(sessions.c.id == session_key(ctx, identity))
                .values(record=result.model_dump(mode="json"))
            )
            return result
