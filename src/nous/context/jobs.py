"""Durable context preparation stores scope once and delegates scheduling to DBOS."""

from uuid import uuid4

from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.context import ContextPreparation, ContextPreparationJob, ContextPrepare
from pydantic import JsonValue
from sqlalchemy import select

from nous.platform.execution import DurableExecution, JobSpec

from .lifecycle import ContextLifecycle
from .models import preparations


class DurableContexts(ContextLifecycle):
    execution: DurableExecution | None

    async def submit_prepare(
        self, ctx: CallContext, command: ContextPrepare
    ) -> ContextPreparationJob:
        if self.execution is None:
            raise DomainError(
                "context_execution_unavailable", "Context execution owner is unavailable"
            )
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="context.submit_prepare",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return ContextPreparationJob.model_validate(tx.replay_result)
            request_id = str(uuid4())
            await tx.connection.execute(
                preparations.insert().values(
                    id=request_id,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    track_id="request:" + request_id,
                    record={
                        "job_request": command.model_dump(mode="json"),
                        "context": ctx.model_dump(mode="json"),
                        "caller_id": ctx.caller_id,
                    },
                )
            )
            job = await tx.enqueue(
                JobSpec(
                    workflow_name="nous.context.prepare", queue_name="default", args=(request_id,)
                )
            )
            return tx.result(ContextPreparationJob(request_id=request_id, job_id=job.workflow_id))

    async def run_prepare(self, request_id: str) -> JsonValue:
        async with self.authority.read() as view:
            raw = await view.connection.scalar(
                select(preparations.c.record).where(preparations.c.id == request_id)
            )
        if raw is None or "job_request" not in raw:
            raise DomainError(
                "context_request_not_found", "Context preparation request is unavailable"
            )
        ctx = CallContext.model_validate(raw["context"])
        command = ContextPrepare.model_validate(raw["job_request"])
        command = command.model_copy(update={"operation_id": "prepare:" + request_id})
        result = await self.prepare(ctx, command)
        # The scheduler retains identities. Source-derived text stays in the context
        # owner records, where source retirement can erase it transactionally.
        return {"preparation_id": result.id}

    async def prepare(self, ctx: CallContext, command: ContextPrepare) -> ContextPreparation:
        raise NotImplementedError

    async def ensure_residency_schedule(self, ctx: CallContext, session_id: str) -> None:
        if self.execution is None:
            return
        from .lifecycle import session_key

        identity = "resident:" + session_key(ctx, session_id)
        async with self.authority.write(
            ctx,
            identity,
            operation_kind="context.resident_schedule",
            scope=ctx.require_subject(),
            payload={"session_id": session_id},
        ) as tx:
            if not tx.replayed:
                await tx.connection.execute(
                    preparations.insert().values(
                        id=identity,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.subject_id,
                        track_id=identity,
                        record={
                            "maintenance_session_id": session_id,
                            "context": ctx.model_dump(mode="json"),
                            "caller_id": ctx.caller_id,
                        },
                    )
                )
                tx.result({"schedule_id": identity})
        if await self.execution.client.get_schedule_async(identity) is None:
            await self.execution.client.create_schedule_async(
                schedule_name=identity,
                workflow_name="nous.context.resident_tick",
                schedule="0 * * * *",
                context={"request_id": identity},
                queue_name="default",
                cron_timezone="UTC",
                automatic_backfill=False,
            )

    async def resident_tick(self, scheduled_at: str, context: JsonValue) -> JsonValue:
        if self.execution is None:
            raise DomainError(
                "context_execution_unavailable", "Context execution owner is unavailable"
            )
        if not isinstance(context, dict) or not isinstance(
            request_id := context.get("request_id"), str
        ):
            raise DomainError(
                "context_schedule_invalid", "Resident schedule requires its saved request"
            )
        async with self.authority.read() as view:
            raw = await view.connection.scalar(
                select(preparations.c.record).where(preparations.c.id == request_id)
            )
        if raw is None:
            await self.execution.client.delete_schedule_async(request_id)
            return {"closed": True}
        ctx = CallContext.model_validate(raw["context"])
        session_id = raw["maintenance_session_id"]
        if not isinstance(session_id, str):
            raise DomainError("context_schedule_invalid", "Resident schedule has no session")
        async with self.authority.read(ctx) as view:
            session = await self.session_in(view, ctx, session_id)
        if session.closed:
            await self.execution.client.delete_schedule_async(request_id)
            return {"closed": True}
        async with self.authority.write(
            ctx,
            request_id + ":" + scheduled_at,
            operation_kind="context.resident_obligation",
            scope=ctx.require_subject(),
            payload={"request_id": request_id, "scheduled_at": scheduled_at},
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            job = await tx.enqueue(
                JobSpec(
                    workflow_name="nous.context.maintain",
                    queue_name="default",
                    args=(request_id, scheduled_at),
                )
            )
            return tx.result({"job_id": job.workflow_id})

    async def run_maintenance(self, request_id: str, scheduled_at: str) -> JsonValue:
        async with self.authority.read() as view:
            raw = await view.connection.scalar(
                select(preparations.c.record).where(preparations.c.id == request_id)
            )
        if raw is None:
            raise DomainError(
                "context_request_not_found", "Resident maintenance request is unavailable"
            )
        ctx = CallContext.model_validate(raw["context"])
        identity = raw["maintenance_session_id"]
        if not isinstance(identity, str):
            raise DomainError("context_schedule_invalid", "Resident schedule has no session")
        result = await self.maintain_session(
            ctx, identity, request_id + ":maintain:" + scheduled_at
        )
        return {"session_id": result.id, "revision": result.revision}
