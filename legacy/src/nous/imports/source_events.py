"""Processing origin captures a new Import event while preserving frozen source facts."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    ImportDocument,
    ImportUnit,
    MediaReference,
    ProcessCatalog,
    TimePoint,
    TimeRange,
)
from nous.platform.call_context import CallContext
from pydantic import TypeAdapter
from sqlalchemy import select, update

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
) -> tuple[SourceRecord, CallContext]:
    if unit.observation_id is None:
        raise ValueError("Frozen ImportUnit must identify its accepted Observation")
    if command.origin is None:
        async with self.authority.read(ctx) as view:
            value = (
                await require_observation_in(
                    view, ctx, unit.observation_id, region_id=unit.source_region_id
                )
            )["record"]
            return value, ctx
    async with self.authority.write(ctx) as tx:
        event_key = f"import-origin:{command.operation_id}:{unit.id}"
        existing = await tx.connection.scalar(
            select(observations.c.record).where(
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.caller_id == ctx.caller_id,
                observations.c.external_event_id == event_key,
            )
        )
        if existing is not None:
            if existing["origin"] != command.origin:
                from nous.platform.errors import DomainError

                raise DomainError(
                    "import_origin_conflict", "Processing run has different source origin"
                )
            restored = await self.materials.restore_input_in(
                tx, ctx, observation_ids=(existing["id"],)
            )
            await require_observation_in(
                tx, restored.context, existing["id"], region_id=unit.source_region_id
            )
            return TypeAdapter(SourceRecord).validate_python(existing), restored.context
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
        if "validity" in previous:
            captured = captured.model_copy(
                update={
                    "validity": TimeRange.model_validate(previous["validity"])
                    if previous["validity"] is not None
                    else None
                }
            )
        else:
            captured = ImportDocument.model_validate(captured.model_dump(exclude={"validity"}))
        selected = (
            MediaReference(
                kind="source_region", id=unit.source_region_id, observation_id=unit.observation_id
            )
            if unit.source_region_id
            else MediaReference(
                kind="material", id=unit.material_id, observation_id=unit.observation_id
            )
        )
        admission = await self.materials.admit_observation_in(
            tx, ctx, unit.material_id, captured, sources=(selected,)
        )
        event, effective = admission.event, admission.context
        event["previous_observation_id"] = previous["id"]
        if command.retry_parent_job_id:
            event["retry_parent_job_id"] = command.retry_parent_job_id
        await tx.connection.execute(
            update(observations).where(observations.c.namespace_id == ctx.namespace_id, observations.c.id == event['id'])
            .values(record=event, caller_id=ctx.caller_id, external_event_id=event_key)
        )
        await require_observation_in(tx, effective, event["id"], region_id=unit.source_region_id)
        return event, effective
