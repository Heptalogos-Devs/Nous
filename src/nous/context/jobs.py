"""Resident maintenance reuses the existing DBOS execution owner."""

from nous.platform.call_context import CallContext
from nous.platform.execution import JobSpec
from .lifecycle import ContextLifecycle, session_key


class DurableContexts(ContextLifecycle):
    async def ensure_residency_schedule(self, ctx: CallContext, session_id: str) -> None:
        identity = "resident:" + session_key(ctx, session_id)
        if await self.execution.client.get_schedule_async(identity) is None:
            await self.execution.client.create_schedule_async(
                schedule_name=identity,
                workflow_name="nous.context.resident_tick",
                schedule="0 * * * *",
                context={"context": ctx.model_dump(mode="json"), "session_id": session_id},
                queue_name="default",
                cron_timezone="UTC",
                automatic_backfill=False,
            )

    async def resident_tick(self, scheduled_at: str, context):
        ctx = CallContext.model_validate(context["context"])
        session_id = context["session_id"]
        identity = "resident:" + session_key(ctx, session_id)
        session = await self.read_session(ctx, session_id)
        if session.closed:
            await self.execution.client.delete_schedule_async(identity)
            return {"closed": True}
        async with self.authority.write(ctx) as tx:
            job = await tx.enqueue(
                JobSpec(
                    "nous.context.maintain",
                    "default",
                    args=(context, scheduled_at),
                    workflow_id=identity + ":" + scheduled_at,
                    scope=session_id,
                )
            )
        return {"job_id": job.workflow_id}

    async def run_maintenance(self, context, scheduled_at: str):
        ctx = CallContext.model_validate(context["context"])
        result = await self.maintain_session(ctx, context["session_id"], "resident:" + scheduled_at)
        return {"session_id": result.id, "revision": result.revision}
