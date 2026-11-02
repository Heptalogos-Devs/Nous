"""Authorize an accepted source event through its actual original or derived selection."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    CognitiveTimes,
    MediaReference,
    TimePoint,
    TimeRange,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .service import MaterialRecord, MaterialService, SourceRecord


@dataclass(frozen=True)
class AcceptedSource:
    event: SourceRecord
    source: MediaReference
    material: MaterialRecord
    region_id: str | None


def cognitive_times(
    event: SourceRecord,
    formation: TimePoint,
    *,
    inferred_event: TimePoint | None = None,
    inferred_validity: TimeRange | None = None,
) -> CognitiveTimes:
    return CognitiveTimes(
        event=TimePoint.model_validate(event["event_time"])
        if event.get("event_time")
        else inferred_event,
        observation=TimePoint.model_validate(event["observed_at"]),
        formation=formation,
        recorded=formation,
        validity=(
            TimeRange.model_validate(event["validity"]) if event["validity"] is not None else None
        )
        if "validity" in event
        else inferred_validity,
    )


async def accepted_source_in(
    self: MaterialService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    observation_id: str,
    source: MediaReference | None = None,
) -> AcceptedSource:
    from .service import (
        MaterialRecord,
        SourceRecord,
        materials,
        observations,
        require_observation_in,
    )

    raw = await view.connection.scalar(
        select(observations.c.record).where(
            observations.c.id == observation_id,
            observations.c.namespace_id == ctx.namespace_id,
            observations.c.active.is_(True),
        )
    )
    if raw is None or raw.get("purged"):
        raise DomainError("source_unavailable", "The accepted source event is unavailable")
    event = TypeAdapter(SourceRecord).validate_python(raw)
    contact = (
        MediaReference.model_validate(raw["contact_source"]) if raw.get("contact_source") else None
    )
    source = (
        source
        or contact
        or MediaReference(kind="material", id=event["material_id"], observation_id=observation_id)
    )
    if contact is not None:
        if (
            source.kind != contact.kind
            or source.id != contact.id
            or source.observation_id
            not in {
                None,
                observation_id,
                contact.observation_id,
            }
        ):
            raise DomainError("observation_source_mismatch", "Use the exact encountered selection")
        source = contact
    region_id = source.id if source.kind == "source_region" else None
    if source.kind in {"representation", "derived_region"}:
        info = await self.provenance_in(view, ctx, source)
        if event["material_id"] not in info.material_ids or (
            contact is None
            and observation_id not in info.roots
            and event.get("previous_observation_id") not in info.roots
        ):
            raise DomainError(
                "observation_source_mismatch", "Representation belongs to a different source event"
            )
        material = TypeAdapter(MaterialRecord).validate_python(
            await view.connection.scalar(
                select(materials.c.record).where(
                    materials.c.id == event["material_id"],
                    materials.c.namespace_id == ctx.namespace_id,
                )
            )
        )
    else:
        if contact is None:
            if source.observation_id not in {
                None,
                observation_id,
                event.get("previous_observation_id"),
            }:
                raise DomainError(
                    "observation_source_mismatch",
                    "Use the accepted event or its recorded predecessor",
                )
            await require_observation_in(view, ctx, observation_id, region_id=region_id)
            source = source.model_copy(update={"observation_id": observation_id})
        material, _, _ = await self.media_source_in(view, ctx, source)
        if material["id"] != event["material_id"]:
            raise DomainError("observation_source_mismatch", "Source belongs to another original")
    return AcceptedSource(event, source, material, region_id)
