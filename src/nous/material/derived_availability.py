"""Content dependency state for source withdrawal and permanent erasure decisions."""

from typing import Literal

from nous_wave.contracts import CallContext, MediaReference
from nous_wave.contracts.derivation import DerivedRegion, DerivedRepresentation
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

from .derived_access import representation_ref
from .derived_schema import derived_regions, representations
from .derived_views import DerivedViews
from .service import materials, observations, regions

DependencyAvailability = Literal["available", "withdrawn", "purged", "missing"]


class DerivedAvailability(DerivedViews):
    async def availability_in(
        self,
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
            states = [await self.availability_in(view, ctx, item.source) for item in value.inputs]
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
