"""Output bytes inherit their real source closure instead of becoming new evidence."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError, MediaReference, SourceRegion
from nous_sdk.contracts.identity import IdentityPurpose
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

from .coverage import merge_source_regions

if TYPE_CHECKING:
    from .service import MaterialService


@dataclass(frozen=True)
class MaterialProvenance:
    reference: MediaReference
    roots: tuple[str, ...]
    regions: dict[str, tuple[str, ...]]
    material_ids: tuple[str, ...]


async def provenance_in(
    self: MaterialService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    reference: MediaReference,
    *,
    purpose: IdentityPurpose = "active",
) -> MaterialProvenance:
    from nous_sdk.contracts.derivation import DerivedRegion

    from .derived_access import representation_ref
    from .derived_availability import dependency_availability_in
    from .derived_schema import derived_regions, representations
    from .service import MaterialRecord, materials, observations, regions
    from .writes import require_observation_exists_in

    async def facts(ref: MediaReference) -> tuple[MaterialRecord, SourceRegion | None, str]:
        selected = None
        identifier = ref.id
        if ref.kind == "source_region":
            value = await view.connection.scalar(
                select(regions.c.record).where(
                    regions.c.id == ref.id, regions.c.namespace_id == ctx.namespace_id
                )
            )
            if value is None or value.get("purged"):
                raise DomainError("source_unavailable", "A provenance source region is unavailable")
            selected = SourceRegion.model_validate(value)
            identifier = selected.material_id
        value = await view.connection.scalar(
            select(materials.c.record).where(
                materials.c.id == identifier, materials.c.namespace_id == ctx.namespace_id
            )
        )
        if value is None or value.get("purged"):
            raise DomainError("source_unavailable", "A provenance material is unavailable")
        source = TypeAdapter(MaterialRecord).validate_python(value)
        statement = select(observations.c.id).where(
            observations.c.material_id == identifier,
            observations.c.namespace_id == ctx.namespace_id,
            observations.c.active.is_(True),
        )
        if ref.observation_id:
            statement = statement.where(
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.id == ref.observation_id,
            )
        source_event = await view.connection.scalar(statement.order_by(observations.c.id).limit(1))
        if source_event is None:
            raise DomainError("source_unavailable", "A provenance source event is unavailable")
        return source, selected, str(source_event)

    async def visit(
        ref: MediaReference, seen: frozenset[str], pins: tuple[str, ...] = ()
    ) -> MaterialProvenance:
        if ref.kind in {"representation", "derived_region"}:
            state = await dependency_availability_in(view, ctx, ref)
            if state != "available":
                raise DomainError(
                    "source_unavailable", "A declared derivation input is unavailable", state=state
                )
            from .access import require_dependencies_in

            await require_dependencies_in(self, view, ctx, (ref.exact(),))
            if ref.kind == "derived_region":
                raw = await view.connection.scalar(
                    select(derived_regions.c.record).where(
                        derived_regions.c.id == ref.id,
                        derived_regions.c.namespace_id == ctx.namespace_id,
                    )
                )
                selected_region = DerivedRegion.model_validate(raw)
                selected_ref = selected_region.representation
            else:
                selected_ref = representation_ref(ref)
            from nous_sdk.contracts.derivation import DerivedRepresentation

            raw = await view.connection.scalar(
                select(representations.c.record).where(
                    representations.c.namespace_id == ctx.namespace_id,
                    representations.c.id == selected_ref.id,
                    representations.c.revision == selected_ref.revision,
                )
            )
            derived = DerivedRepresentation.model_validate(raw)
            return MaterialProvenance(
                ref, derived.source_roots, derived.source_regions, derived.material_ids
            )
        source, selected, source_event = await facts(ref)
        material_id = source["id"]
        if material_id in seen:
            raise DomainError("source_cycle", "Artifact provenance cannot depend on itself")
        dependencies = tuple(source.get("required_source_ids", ()))
        own_pins = tuple(source.get("required_observation_ids", ()))
        for pin in own_pins:
            await require_observation_exists_in(view, ctx, pin)
        if not dependencies:
            matching = (
                tuple(
                    (
                        await view.connection.execute(
                            select(observations.c.id).where(
                                observations.c.id.in_(pins),
                                observations.c.material_id == material_id,
                                observations.c.namespace_id == ctx.namespace_id,
                                observations.c.active.is_(True),
                            )
                        )
                    ).scalars()
                )
                if pins and ref.observation_id is None
                else ()
            )
            leaf_roots = matching or (source_event,)
            return MaterialProvenance(
                ref.model_copy(update={"observation_id": source_event}),
                leaf_roots,
                {root: (selected.id,) for root in leaf_roots} if selected else {},
                (material_id,),
            )
        roots: set[str] = set()
        material_ids: set[str] = {material_id}
        requirements: list[tuple[str, tuple[str, ...]]] = []

        def merge(info: MaterialProvenance) -> None:
            roots.update(info.roots)
            material_ids.update(info.material_ids)
            for root in info.roots:
                requirements.append((root, info.regions.get(root, ())))

        for locator in dependencies:
            kind, colon, identity = locator.partition(":")
            if colon and kind in {
                "material",
                "source-region",
                "source_region",
                "representation",
                "derived-region",
                "derived_region",
            }:
                media = MediaReference.model_validate(
                    {"kind": kind.replace("-", "_"), "id": identity}
                )
                merge(await visit(media, seen | {material_id}, own_pins))
                continue
            # Bare identities identify a source material, region or Observation.
            pin = locator.removeprefix("observation:")
            source_row = (
                await view.connection.execute(
                    select(observations.c.material_id).where(
                        observations.c.id == pin,
                        observations.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).scalar_one_or_none()
            if source_row is not None:
                await require_observation_exists_in(view, ctx, pin)
                merge(MaterialProvenance(reference, (pin,), {}, (str(source_row),)))
                continue
            from .service import regions

            region_material = await view.connection.scalar(
                select(regions.c.material_id).where(
                    regions.c.id == locator, regions.c.namespace_id == ctx.namespace_id
                )
            )
            media = MediaReference(
                kind="source_region" if region_material is not None else "material", id=locator
            )
            merge(await visit(media, seen | {material_id}, own_pins))
        # Explicit pins are provenance constraints, not a fresh observation of the output bytes.
        for pin in own_pins:
            source_id = await view.connection.scalar(
                select(observations.c.material_id).where(
                    observations.c.id == pin,
                    observations.c.namespace_id == ctx.namespace_id,
                )
            )
            if source_id is not None:
                material_ids.add(str(source_id))
            roots.add(pin)
        return MaterialProvenance(
            ref,
            tuple(sorted(roots)),
            merge_source_regions(requirements),
            tuple(sorted(material_ids)),
        )

    if reference.kind in {"representation", "derived_region"}:
        return await visit(reference, frozenset())
    material, region, event = await self.media_source_in(view, ctx, reference, purpose=purpose)
    if material.get("required_source_ids"):
        return await visit(reference.model_copy(update={"observation_id": event}), frozenset())
    return MaterialProvenance(
        reference.model_copy(update={"observation_id": event}),
        (event,),
        {event: (region.id,)} if region else {},
        (material["id"],),
    )
