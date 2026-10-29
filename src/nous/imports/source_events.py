"""Processing origin captures a new Import event while preserving frozen source facts."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, ImportDocument, ImportUnit, ProcessCatalog, TimePoint
from pydantic import TypeAdapter
from sqlalchemy import update

from nous.material.service import SourceRecord, observations, require_observation_in
from nous.platform.execution import require_job_in

if TYPE_CHECKING:
    from .service import ImportService


async def processing_source(
    self: ImportService,
    ctx: CallContext,
    unit: ImportUnit,
    command: ProcessCatalog,
    document: ImportDocument,
) -> SourceRecord:
    if unit.observation_id is None:
        raise ValueError("Frozen ImportUnit must identify its accepted Observation")
    if command.origin is None:
        async with self.authority.read(ctx) as view:
            return (
                await require_observation_in(
                    view, ctx, unit.observation_id, region_id=unit.source_region_id
                )
            )["record"]
    async with self.authority.write(
        ctx,
        f"{command.operation_id}:{unit.id}:origin",
        operation_kind="imports.processing_origin",
        scope=ctx.require_subject(),
        payload={"command": command.model_dump(mode="json"), "unit_id": unit.id},
    ) as tx:
        if tx.replayed:
            record = TypeAdapter(SourceRecord).validate_python(tx.replay_result)
            await require_observation_in(tx, ctx, record["id"], region_id=unit.source_region_id)
            return record
        if command.retry_parent_job_id:
            await require_job_in(tx, ctx, command.retry_parent_job_id)
        previous = (
            await require_observation_in(
                tx, ctx, unit.observation_id, region_id=unit.source_region_id
            )
        )["record"]
        captured = document.model_copy(
            update={
                "origin": command.origin,
                "source_type": previous["source_type"],
                "source": previous.get("source"),
                "observed_at": TimePoint.model_validate(previous["observed_at"]),
                "event_time": TimePoint.model_validate(previous["event_time"])
                if previous["event_time"]
                else None,
            }
        )
        event = await self.materials.observe_in(tx, ctx, unit.material_id, captured)
        event["previous_observation_id"] = previous["id"]
        if command.retry_parent_job_id:
            event["retry_parent_job_id"] = command.retry_parent_job_id
        await tx.connection.execute(
            update(observations).where(observations.c.id == event["id"]).values(record=event)
        )
        await require_observation_in(tx, ctx, event["id"], region_id=unit.source_region_id)
        return tx.result(event)
