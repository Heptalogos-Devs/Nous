"""Content dependency state for source withdrawal and permanent erasure decisions."""

from typing import Literal

from nous_sdk.contracts import CallContext, CognitiveRecord, CognitiveRef, MediaReference
from nous_sdk.contracts.derivation import DerivedRegion, DerivedRepresentation
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

from .derived_access import representation_ref
from .derived_schema import derived_regions, representations
from .derived_views import DerivedViews
from .service import materials, observations, regions

DependencyAvailability = Literal["available", "withdrawn", "purged", "missing"]


async def dependency_availability_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    reference: MediaReference,
) -> DependencyAvailability:
    """Inspect same-subject dependencies for domain maintenance, without returning source bodies."""
    if reference.kind in {"representation", "derived_region"}:
        if reference.kind == "derived_region":
            raw_region = await view.connection.scalar(
                select(derived_regions.c.record).where(
                    derived_regions.c.id == reference.id,
                    derived_regions.c.namespace_id == ctx.namespace_id,
                    derived_regions.c.subject_id == ctx.require_subject(),
                )
            )
            if raw_region is None:
                return "missing"
            region = DerivedRegion.model_validate(raw_region)
            if region.purged:
                return "purged"
            ref = region.representation
        else:
            ref = representation_ref(reference)
        raw = await view.connection.scalar(
            select(representations.c.record).where(
                representations.c.id == ref.id,
                representations.c.revision == ref.revision,
                representations.c.namespace_id == ctx.namespace_id,
                representations.c.subject_id == ctx.require_subject(),
            )
        )
        if raw is None:
            return "missing"
        value = DerivedRepresentation.model_validate(raw)
        if value.purged or value.payload is None:
            return "purged"
        if reference.observation_id and reference.observation_id not in value.source_roots:
            return "missing"
        states = [await dependency_availability_in(view, ctx, item.source) for item in value.inputs]
        for unavailable in ("purged", "missing", "withdrawn"):
            if unavailable in states:
                return unavailable
        material_ids, roots = value.material_ids, value.source_roots
    else:
        material_id = reference.id
        if reference.kind == "source_region":
            material_id = await view.connection.scalar(
                select(regions.c.material_id).where(
                    regions.c.id == reference.id,
                    regions.c.namespace_id == ctx.namespace_id,
                )
            )
            if material_id is None:
                return "missing"
        material_ids = (material_id,)
        roots = tuple(
            str(identity)
            for identity in (
                await view.connection.execute(
                    select(observations.c.id).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.require_subject(),
                        observations.c.material_id == material_id,
                    )
                )
            ).scalars()
            if reference.observation_id is None or identity == reference.observation_id
        )
    material_rows = (
        (
            await view.connection.execute(
                select(materials.c.record).where(
                    materials.c.namespace_id == ctx.namespace_id,
                    materials.c.id.in_(material_ids),
                )
            )
        )
        .scalars()
        .all()
    )
    if len(material_rows) != len(material_ids):
        return "missing"
    if any(row.get("purged") for row in material_rows):
        return "purged"
    from nous.cognition.schema import revisions

    for material in material_rows:
        for locator in material.get("required_source_ids", ()):
            kind, colon, identity = str(locator).partition(":")
            if kind in {
                "material",
                "source-region",
                "source_region",
                "representation",
                "derived-region",
                "derived_region",
            }:
                state = await dependency_availability_in(
                    view,
                    ctx,
                    MediaReference.model_validate({"kind": kind.replace("-", "_"), "id": identity}),
                )
                if state != "available":
                    return state
            elif colon and "@" in identity:
                identifier, revision = identity.rsplit("@", 1)
                cognitive_reference = TypeAdapter(CognitiveRef).validate_python(
                    {"kind": kind, "id": identifier, "revision": int(revision)}
                )
                raw = await view.connection.scalar(
                    select(revisions.c.record).where(
                        revisions.c.id == cognitive_reference.id,
                        revisions.c.revision == cognitive_reference.revision,
                        revisions.c.kind == cognitive_reference.kind,
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.subject_id == ctx.require_subject(),
                    )
                )
                if raw is None:
                    return "missing"
                record = CognitiveRecord.model_validate(raw)
                if record.purged:
                    return "purged"
                active = set(
                    (
                        await view.connection.execute(
                            select(observations.c.id).where(
                                observations.c.namespace_id == ctx.namespace_id,
                                observations.c.subject_id == ctx.require_subject(),
                                observations.c.id.in_(record.support_roots or record.source_roots),
                                observations.c.active.is_(True),
                            )
                        )
                    ).scalars()
                )
                if not active:
                    return "withdrawn"
            elif not colon:
                known = await view.connection.scalar(
                    select(materials.c.id).where(
                        materials.c.id == locator, materials.c.namespace_id == ctx.namespace_id
                    )
                )
                if known:
                    state = await dependency_availability_in(
                        view, ctx, MediaReference(kind="material", id=str(locator))
                    )
                    if state != "available":
                        return state
    required_pins = {
        pin for row in material_rows for pin in row.get("required_observation_ids", ())
    }
    if not roots:
        return "missing"
    rows = (
        (
            await view.connection.execute(
                select(observations).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.subject_id == ctx.require_subject(),
                    observations.c.id.in_((*roots, *required_pins)),
                )
            )
        )
        .mappings()
        .all()
    )
    if len(rows) != len(set(roots) | required_pins):
        return "missing"
    if any(row["record"].get("purged") for row in rows if row["id"] in required_pins):
        return "purged"
    if any(not row["active"] for row in rows if row["id"] in required_pins):
        return "withdrawn"
    if reference.kind in {"material", "source_region"} and reference.observation_id is None:
        return (
            "available"
            if any(row["active"] and row["id"] in roots for row in rows)
            else "withdrawn"
        )
    if any(row["record"].get("purged") for row in rows):
        return "purged"
    return "available" if all(row["active"] for row in rows) else "withdrawn"


class DerivedAvailability(DerivedViews):
    availability_in = staticmethod(dependency_availability_in)
