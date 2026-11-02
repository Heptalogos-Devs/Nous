"""Output artifacts retain the exact authorized dependency closure of their bytes."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import MediaReference, SourceRegion
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.identity import IdentityPurpose
from sqlalchemy import select

from nous.identity.access import require_namespace_in
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
    from nous_sdk.contracts.derivation import DerivedRegion, DerivedRepresentation
    from .derived_access import representation_ref
    from .derived_availability import dependency_availability_in
    from .derived_schema import derived_regions, representations
    from .service import materials, observations, regions, require_observation_in

    async def authorize_media(reference: MediaReference, selected_ctx: CallContext) -> None:
        if reference.kind in {"material", "source_region"}:
            if reference.kind == "material":
                await read_material_in(
                    service, view, selected_ctx, reference.id, seen, reference.observation_id
                )
            else:
                await service.media_source_in(view, selected_ctx, reference)
            return
        state = await dependency_availability_in(view, selected_ctx, reference)
        if state != "available":
            raise DomainError(
                "source_unavailable", "A required derived input is unavailable", state=state
            )
        if reference.kind == "derived_region":
            raw = await view.connection.scalar(
                select(derived_regions.c.record).where(
                    derived_regions.c.namespace_id == selected_ctx.namespace_id,
                    derived_regions.c.id == reference.id,
                )
            )
            parent = DerivedRegion.model_validate(raw).representation
        else:
            parent = representation_ref(reference)
        raw = await view.connection.scalar(
            select(representations.c.record).where(
                representations.c.namespace_id == selected_ctx.namespace_id,
                representations.c.id == parent.id,
                representations.c.revision == parent.revision,
            )
        )
        value = DerivedRepresentation.model_validate(raw)
        for group in selected_ctx.source_groups():
            if reference.id in group or reference.exact() in group:
                continue
            scoped = selected_ctx.for_source_group(group)
            for item in value.inputs:
                await authorize_media(item.source, scoped)

    for locator in required:
        if locator.startswith("material:"):
            await read_material_in(service, view, ctx, locator.removeprefix("material:"), seen)
            continue
        if ":" in locator:
            kind, _, identifier = locator.partition(":")
            if kind == "observation":
                await require_observation_in(view, ctx, identifier)
            elif kind.replace("-", "_") in {
                "material",
                "source_region",
                "representation",
                "derived_region",
            }:
                await authorize_media(
                    MediaReference.model_validate(
                        {"kind": kind.replace("-", "_"), "id": identifier}
                    ),
                    ctx,
                )
            else:
                raise DomainError(
                    "source_dependency_invalid",
                    "Material dependencies must identify actual source inputs",
                )
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
    observation_id: str | None = None,
    *,
    purpose: IdentityPurpose = "active",
) -> MaterialRecord:
    from .service import (
        MaterialRecord,
        materials,
        observations,
        source_permitted,
    )

    await require_namespace_in(view, ctx, purpose=purpose)
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
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.active.is_(True),
                )
            )
        )
        .scalars()
        .all()
    )
    if observation_id is not None:
        events = [event for event in events if event == observation_id]
    if not events:
        raise DomainError("source_unavailable", "Source has no live contact in this subject")
    from .writes import require_observation_exists_in

    for dependency in value.get("required_observation_ids", ()):
        await require_observation_exists_in(view, ctx, dependency)
    required = tuple(value.get("required_source_ids", ()))
    if required:
        from .derived_availability import dependency_availability_in

        state = await dependency_availability_in(
            view,
            ctx,
            MediaReference(kind="material", id=material_id, observation_id=observation_id),
        )
        if state != "available":
            raise DomainError(
                "source_unavailable", "An artifact dependency is unavailable", state=state
            )
        permitted = False
        for event in events:
            for group in ctx.source_groups():
                scoped = ctx.for_source_group(group)
                if source_permitted(scoped, material_id, event):
                    continue
                try:
                    await require_dependencies_in(
                        service, view, scoped, required, seen | {material_id}
                    )
                except DomainError as error:
                    if error.code not in {
                        "source_unavailable",
                        "cognition_not_found",
                        "reference_not_found",
                        "region_not_found",
                    }:
                        raise
                    break
            else:
                permitted = True
                break
        if not permitted:
            raise DomainError(
                "source_unavailable",
                "Artifact dependencies are outside the current source constraints",
            )
    elif not any(source_permitted(ctx, material_id, observation_id) for observation_id in events):
        raise DomainError("source_unavailable", "Source is outside the current read scope")
    from pydantic import TypeAdapter

    return TypeAdapter(MaterialRecord).validate_python(value)


async def media_source_in(
    self: MaterialService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    reference: MediaReference,
    *,
    purpose: IdentityPurpose = "active",
) -> tuple[MaterialRecord, SourceRegion | None, str]:
    """Authorize an exact original/region and honor an explicitly pinned observation."""
    from pydantic import TypeAdapter

    from .service import MaterialRecord, materials, observations, require_observation_in

    region: SourceRegion | None = None
    if reference.kind == "material":
        material = await read_material_in(
            self,
            view,
            ctx,
            reference.id,
            observation_id=reference.observation_id,
            purpose=purpose,
        )
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
