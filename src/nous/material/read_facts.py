"""Source dependency metadata belongs to one read transaction and one actual scope."""

from dataclasses import dataclass
from typing import TypedDict

from nous_sdk.contracts import CallContext, MediaReference
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx


class MaterialFacts(TypedDict):
    purged: bool
    required_source_ids: list[str]
    required_observation_ids: list[str]


@dataclass(frozen=True)
class ObservationFacts:
    id: str
    material_id: str
    kind: str
    active: bool
    purged: bool
    contact_source: MediaReference | None


@dataclass(frozen=True)
class SourceReadFacts:
    observations: dict[str, ObservationFacts]
    material_ids: frozenset[str]
    materials: dict[str, MaterialFacts]
    region_materials: dict[str, str]


async def source_facts_in(view: ReadView | WriteTx, ctx: CallContext) -> SourceReadFacts | None:
    # Writers must see changes made earlier in their own transaction. Connection
    # pool state is deliberately unused: a new ReadView always reads fresh facts.
    if not isinstance(view, ReadView):
        return None
    from .service import materials, observations, regions

    key = (
        "material.source_facts",
        ctx.caller_id,
        ctx.namespace_id,
        ctx.namespace_id,
        ctx.source_groups(),
    )
    saved = view.facts.get(key)
    if isinstance(saved, SourceReadFacts):
        return saved
    events = {
        str(row.id): ObservationFacts(
            str(row.id),
            str(row.material_id),
            str(row.kind),
            bool(row.active),
            bool(row.purged),
            MediaReference.model_validate(row.contact_source)
            if row.contact_source is not None
            else None,
        )
        for row in (
            await view.connection.execute(
                select(
                    observations.c.id,
                    observations.c.material_id,
                    observations.c.kind,
                    observations.c.active,
                    observations.c.record["purged"].as_boolean().label("purged"),
                    observations.c.record["contact_source"].label("contact_source"),
                ).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    
                )
            )
        ).all()
    }
    ids = frozenset(event.material_id for event in events.values())
    values: dict[str, MaterialFacts] = {}
    for row in (
        await view.connection.execute(
            select(
                materials.c.id,
                materials.c.record["purged"].as_boolean().label("purged"),
                materials.c.record["required_source_ids"].label("sources"),
                materials.c.record["required_observation_ids"].label("pins"),
            ).where(materials.c.namespace_id == ctx.namespace_id, materials.c.id.in_(ids))
        )
    ).all():
        values[str(row.id)] = MaterialFacts(
            purged=bool(row.purged),
            required_source_ids=[str(value) for value in (row.sources or ())],
            required_observation_ids=[str(value) for value in (row.pins or ())],
        )
    source_regions = {
        str(row.id): str(row.material_id)
        for row in (
            await view.connection.execute(
                select(regions.c.id, regions.c.material_id).where(
                    regions.c.namespace_id == ctx.namespace_id, regions.c.material_id.in_(ids)
                )
            )
        ).all()
    }
    result = SourceReadFacts(events, ids, values, source_regions)
    view.facts[key] = result
    return result
