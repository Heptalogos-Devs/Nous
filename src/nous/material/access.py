"""Output artifacts retain the exact authorized dependency closure of their bytes."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, DomainError, MediaReference, SourceRegion
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .service import MaterialRecord, MaterialService


async def require_dependencies_in(
    service: MaterialService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    required: tuple[str, ...],
    seen: frozenset[str] = frozenset(),
) -> None:
    from nous.cognition.service import CognitionService
    from nous.references.service import ReferenceDirectory

    from .service import materials, observations, regions, require_observation_in

    directory = ReferenceDirectory(service.authority, CognitionService(service.authority), service)
    for locator in required:
        if locator.startswith("material:"):
            await read_material_in(service, view, ctx, locator.removeprefix("material:"), seen)
            continue
        if ":" in locator:
            resolved = await directory.resolve_in(view, ctx, locator)
            reference = resolved.reference.reference
            if reference.kind == "material":
                await read_material_in(service, view, ctx, reference.id, seen)
            continue
        material = await view.connection.scalar(
            select(materials.c.id).where(
                materials.c.id == locator, materials.c.namespace_id == ctx.namespace_id
            )
        )
        if material is not None:
            await read_material_in(service, view, ctx, locator, seen)
            continue
        region = await view.connection.scalar(
            select(regions.c.id).where(
                regions.c.id == locator, regions.c.namespace_id == ctx.namespace_id
            )
        )
        if region is not None:
            await service.read_region_in(view, ctx, locator)
            continue
        observation = await view.connection.scalar(
            select(observations.c.id).where(
                observations.c.id == locator,
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.subject_id == ctx.subject_id,
            )
        )
        if observation is not None:
            await require_observation_in(view, ctx, locator)
            continue
        raise DomainError(
            "source_unavailable", "An artifact dependency is unavailable in the current scope"
        )


async def read_material_in(
    service: MaterialService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    material_id: str,
    seen: frozenset[str] = frozenset(),
) -> MaterialRecord:
    from .service import MaterialRecord, materials, observations, source_permitted

    await require_subject_in(view, ctx)
    if material_id in seen:
        raise DomainError("source_cycle", "An artifact cannot depend on itself")
    value = await view.connection.scalar(
        select(materials.c.record).where(
            materials.c.id == material_id, materials.c.namespace_id == ctx.namespace_id
        )
    )
    if value is None or value.get("purged"):
        raise DomainError("source_unavailable", "Source content is unavailable")
    events = (
        (
            await view.connection.execute(
                select(observations.c.id).where(
                    observations.c.material_id == material_id,
                    observations.c.subject_id == ctx.subject_id,
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.active.is_(True),
                )
            )
        )
        .scalars()
        .all()
    )
    if not events:
        raise DomainError("source_unavailable", "Source has no live contact in this subject")
    required = tuple(value.get("required_source_ids", ()))
    if required:
        await require_dependencies_in(service, view, ctx, required, seen | {material_id})
    elif not any(source_permitted(ctx, material_id, observation_id) for observation_id in events):
        raise DomainError("source_unavailable", "Source is outside the current read scope")
    from pydantic import TypeAdapter

    return TypeAdapter(MaterialRecord).validate_python(value)


async def media_source_in(
    self: MaterialService, view: ReadView | WriteTx, ctx: CallContext, reference: MediaReference
) -> tuple[MaterialRecord, SourceRegion | None, str]:
    """Authorize an exact original/region and honor an explicitly pinned observation."""
    from pydantic import TypeAdapter

    from .service import MaterialRecord, materials, observations, require_observation_in

    region: SourceRegion | None = None
    if reference.kind == "material":
        material = await self.read_in(view, ctx, reference.id)
    elif reference.kind == "source_region":
        region = await self.read_region_in(view, ctx, reference.id)
        value = await view.connection.scalar(
            select(materials.c.record).where(
                materials.c.id == region.material_id, materials.c.namespace_id == ctx.namespace_id
            )
        )
        material = TypeAdapter(MaterialRecord).validate_python(value)
    else:
        raise DomainError(
            "source_reference_invalid", "This source owner accepts originals and source regions"
        )
    events = (
        (
            await view.connection.execute(
                select(observations.c.id).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.subject_id == ctx.subject_id,
                    observations.c.material_id == material["id"],
                    observations.c.active.is_(True),
                )
            )
        )
        .scalars()
        .all()
    )
    if reference.observation_id is not None:
        events = [event for event in events if event == reference.observation_id]
    for event in events:
        try:
            if not material.get("required_source_ids"):
                await require_observation_in(
                    view, ctx, event, region_id=region.id if region else None
                )
            return material, region, str(event)
        except DomainError as error:
            if error.code != "source_unavailable":
                raise
    raise DomainError(
        "source_unavailable", "The exact media observation is unavailable in this scope"
    )
