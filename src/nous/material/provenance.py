"""Output bytes inherit their real source closure instead of becoming new evidence."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, CognitiveRef, DomainError, MediaReference
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

from .coverage import merge_source_regions

if TYPE_CHECKING:
    from .derived import DerivedService
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
    derivations: DerivedService | None = None,
) -> MaterialProvenance:
    from nous.cognition.service import CognitionService

    from .service import observations, require_observation_in

    if derivations is None:
        from .derived import DerivedService

        derivations = DerivedService(self.authority, self)
    material, region, event = await self.media_source_in(view, ctx, reference)
    internal = ctx.model_copy(update={"allowed_source_ids": None, "source_constraints": ()})
    cognition = CognitionService(self.authority, materials=self, derivations=derivations)

    async def visit(
        ref: MediaReference, seen: frozenset[str], pins: tuple[str, ...] = ()
    ) -> MaterialProvenance:
        if ref.kind in {"representation", "derived_region"}:
            info = await derivations.source_in(view, internal, ref)
            return MaterialProvenance(info.reference, info.roots, info.regions, info.material_ids)
        source, selected, source_event = await self.media_source_in(view, internal, ref)
        material_id = source["id"]
        if material_id in seen:
            raise DomainError("source_cycle", "Artifact provenance cannot depend on itself")
        dependencies = tuple(source.get("required_source_ids", ()))
        own_pins = tuple(source.get("required_observation_ids", ()))
        for pin in own_pins:
            await require_observation_in(view, internal, pin)
        if not dependencies:
            matching = (
                tuple(
                    (
                        await view.connection.execute(
                            select(observations.c.id).where(
                                observations.c.id.in_(pins),
                                observations.c.material_id == material_id,
                                observations.c.namespace_id == ctx.namespace_id,
                                observations.c.subject_id == ctx.subject_id,
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
            if colon and "@" in identity:
                object_id, revision = identity.rsplit("@", 1)
                record = await cognition.get_in(
                    view,
                    internal,
                    TypeAdapter(CognitiveRef).validate_python(
                        {"kind": kind, "id": object_id, "revision": int(revision)}
                    ),
                )
                merge(MaterialProvenance(reference, record.source_roots, record.source_regions, ()))
                continue
            # Legacy bare dependency identities are material, region or actual Observation IDs.
            pin = locator.removeprefix("observation:")
            source_row = (
                await view.connection.execute(
                    select(observations.c.material_id).where(
                        observations.c.id == pin,
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                    )
                )
            ).scalar_one_or_none()
            if source_row is not None:
                await require_observation_in(view, internal, pin)
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
                select(observations.c.material_id).where(observations.c.id == pin)
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

    if material.get("required_source_ids"):
        return await visit(reference, frozenset())
    return MaterialProvenance(
        reference.model_copy(update={"observation_id": event}),
        (event,),
        {event: (region.id,)} if region else {},
        (material["id"],),
    )
