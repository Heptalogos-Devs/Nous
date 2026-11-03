"""Actual subject contact shares the existing durable growth and adoption pipeline."""

from __future__ import annotations

from typing import TYPE_CHECKING

import sqlalchemy as sa
from nous_sdk.contracts.growth import GrowthEntry, GrowthSnapshot
from nous_sdk.contracts.maintenance import StartMaintenance

from nous.platform.call_context import CallContext
from nous.platform.persistence import WriteTx

if TYPE_CHECKING:
    from .service import MaintenanceService


async def snapshot_in(
    self: MaintenanceService, tx: WriteTx, ctx: CallContext, command: StartMaintenance
) -> GrowthSnapshot:
    current = await self.growth.growth_snapshot_in(tx, ctx, (), 1)
    records = {ref.exact(): await self.cognition.get_in(tx, ctx, ref) for ref in command.references}
    entries = tuple(
        GrowthEntry(key=f"c{i}", record=record) for i, record in enumerate(records.values())
    )
    roots = {root for record in records.values() for root in record.source_roots}
    from nous.material.service import observations

    material_ids = tuple(
        (
            await tx.connection.execute(
                sa.select(observations.c.material_id).where(
                    observations.c.id.in_(roots),
                    observations.c.namespace_id == ctx.namespace_id,
                )
            )
        ).scalars()
    )
    material_sources = ()
    if command.source_catalog_id is not None:
        from nous.cognition.growth_sources import from_units_in

        if self.imports is None:
            raise RuntimeError("Material-backed growth requires the shared import owner")
        source_catalog = await self.imports.read_catalog_in(tx, ctx, command.source_catalog_id)
        material_sources = await from_units_in(self.cognition, tx, ctx, source_catalog.units)
        roots.update(
            link.observation_id
            for source in material_sources
            for link in source.evidence
            if link.observation_id is not None
        )
        material_ids = tuple(
            (
                await tx.connection.execute(
                    sa.select(observations.c.material_id).where(
                        observations.c.id.in_(roots),
                        observations.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).scalars()
        )
    if command.material_sources:
        from nous.cognition.growth_sources import from_references_in

        material_sources = (
            *material_sources,
            *await from_references_in(
                self.cognition, tx, ctx, command.material_sources, start=len(material_sources)
            ),
        )
        for source in material_sources:
            material_ids = (
                *material_ids,
                *(
                    await self.cognition.materials.provenance_in(tx, ctx, source.source)
                ).material_ids,
            )
    return GrowthSnapshot(
        watermark=tx.commit_seq,
        entries=entries,
        material_ids=tuple(sorted(set(material_ids))),
        capabilities=current.capabilities,
        sources=material_sources,
    )
