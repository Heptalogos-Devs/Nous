"""Repeat policies reconcile through public DBOS schedules, without a polling loop."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4
from zoneinfo import ZoneInfo

import sqlalchemy as sa
from croniter import croniter
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.common import utc_now
from nous_sdk.contracts.maintenance import (
    CreateMaintenanceSchedule,
    MaintenanceSchedule,
    StartMaintenance,
)
from pydantic import JsonValue

from nous.identity.service import require_subject_in
from nous.platform.persistence import ReadView, WriteTx

from .schema import schedules

if TYPE_CHECKING:
    from .service import MaintenanceService


def project_schedule(record: MaintenanceSchedule, ctx: CallContext) -> MaintenanceSchedule:
    # A repeat instruction has no catalog of evidence against which a narrower
    # source scope can be proven. Keep policy metadata, withhold its free text.
    if ctx.source_groups():
        return record.model_copy(
            update={"command": record.command.model_copy(update={"instruction": ""})}
        )
    return record


async def schedule_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    identity: str,
) -> MaintenanceSchedule:
    raw = await view.connection.scalar(
        sa.select(schedules.c.record).where(
            schedules.c.id == identity,
            schedules.c.namespace_id == ctx.namespace_id,
            schedules.c.subject_id == ctx.require_subject(),
        )
    )
    if raw is None:
        raise DomainError(
            "maintenance_schedule_not_found", "Maintenance schedule is unavailable in this subject"
        )
    return MaintenanceSchedule.model_validate(raw)


async def create_schedule(
    self: MaintenanceService,
    ctx: CallContext,
    command: CreateMaintenanceSchedule,
) -> MaintenanceSchedule:
    if not croniter.is_valid(command.cron, second_at_beginning=True):
        raise ValueError("Invalid maintenance cron expression")
    ZoneInfo(command.timezone)
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="maintenance.schedule.create",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            identity = str(tx.replay_result)
        else:
            await require_subject_in(tx, ctx)
            identity = "maintenance:" + str(uuid4())
            record = MaintenanceSchedule(
                id=identity,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                created_by=ctx.caller_id,
                created_at=tx.recorded_at,
                name=command.name,
                cron=command.cron,
                timezone=command.timezone,
                command=command.command,
            )
            await tx.connection.execute(
                schedules.insert().values(
                    id=identity,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    context=ctx.model_dump(mode="json"),
                    record=record.model_dump(mode="json"),
                )
            )
            tx.result(identity)
    async with self.authority.read(ctx) as view:
        record = await schedule_in(view, ctx, identity)
    await reconcile_schedule(self, record)
    return project_schedule(record, ctx)


async def list_schedules(
    self: MaintenanceService, ctx: CallContext
) -> tuple[MaintenanceSchedule, ...]:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        rows = (
            await view.connection.execute(
                sa.select(schedules.c.record)
                .where(
                    schedules.c.namespace_id == ctx.namespace_id,
                    schedules.c.subject_id == ctx.subject_id,
                )
                .order_by(schedules.c.id)
            )
        ).scalars()
        return tuple(project_schedule(MaintenanceSchedule.model_validate(raw), ctx) for raw in rows)


async def stop_schedule(
    self: MaintenanceService,
    ctx: CallContext,
    identity: str,
    operation_id: str,
) -> MaintenanceSchedule:
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="maintenance.schedule.stop",
        scope=ctx.require_subject(),
        payload={"schedule_id": identity},
    ) as tx:
        if not tx.replayed:
            record = await schedule_in(tx, ctx, identity)
            if record.stopped_at is None:
                record = record.model_copy(update={"stopped_at": tx.recorded_at})
                await tx.connection.execute(
                    schedules.update()
                    .where(schedules.c.id == identity)
                    .values(record=record.model_dump(mode="json"))
                )
            tx.result(identity)
    async with self.authority.read(ctx) as view:
        record = await schedule_in(view, ctx, identity)
    await reconcile_schedule(self, record)
    return project_schedule(record, ctx)


async def reconcile_schedule(self: MaintenanceService, record: MaintenanceSchedule) -> None:
    if record.stopped_at is not None:
        await self.execution.client.delete_schedule_async(record.id)
        return
    if await self.execution.client.get_schedule_async(record.id) is None:
        await self.execution.client.apply_schedules_async(
            [
                {
                    "schedule_name": record.id,
                    "workflow_name": "nous.maintenance.tick",
                    "schedule": record.cron,
                    "context": {"schedule_id": record.id},
                    "automatic_backfill": False,
                    "cron_timezone": record.timezone,
                    "queue_name": "default",
                }
            ]
        )


async def reconcile_schedules(self: MaintenanceService) -> None:
    async with self.authority.read() as view:
        rows = (await view.connection.execute(sa.select(schedules.c.record))).scalars().all()
    for raw in rows:
        await reconcile_schedule(self, MaintenanceSchedule.model_validate(raw))


async def run_tick(self: MaintenanceService, scheduled_at: str, context: JsonValue) -> JsonValue:
    if not isinstance(context, dict) or not isinstance(identity := context.get("schedule_id"), str):
        raise DomainError(
            "maintenance_schedule_invalid", "A saved maintenance schedule is required"
        )
    async with self.authority.read() as view:
        raw = await view.connection.scalar(
            sa.select(schedules.c.context).where(schedules.c.id == identity)
        )
    if raw is None:
        await self.execution.client.delete_schedule_async(identity)
        return {"stopped": True}
    ctx = CallContext.model_validate(raw).model_copy(update={"clock": utc_now()})
    operation_id = identity + ":" + scheduled_at
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="maintenance.tick",
        scope=ctx.require_subject(),
        payload={"schedule_id": identity, "scheduled_at": scheduled_at},
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        record = await schedule_in(tx, ctx, identity)
        if record.stopped_at is not None:
            return tx.result({"stopped": True})
        command = StartMaintenance(**record.command.model_dump(), operation_id=operation_id)
        submission = await self.submit_in(tx, ctx, command)
        return tx.result(submission.model_dump(mode="json"))
