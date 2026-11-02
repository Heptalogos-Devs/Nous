"""Actual subject contact shares the existing durable growth and adoption pipeline."""

from __future__ import annotations

from typing import TYPE_CHECKING

import sqlalchemy as sa
from nous_sdk.contracts import CallContext, CognitiveRef
from nous_sdk.contracts.growth import GrowthEntry, GrowthSnapshot
from nous_sdk.contracts.maintenance import GrowthPolicy, MaintenanceRun, StartMaintenance

from nous.platform.persistence import WriteTx

from .schema import runs

if TYPE_CHECKING:
    from .service import MaintenanceService


async def on_contact_in(
    self: MaintenanceService,
    tx: WriteTx,
    ctx: CallContext,
    observation_id: str,
    references: tuple[CognitiveRef, ...],
) -> None:
    if not references or self.configuration is None:
        return
    effective = await self.configuration.effective(ctx, "growth")
    policy = GrowthPolicy.model_validate(effective["value"])
    if policy.mode == "off":
        return
    # Coalesce contacts while a queued run has not frozen its evidence directory.
    pending = (
        await tx.connection.execute(
            sa.select(runs.c.record)
            .where(runs.c.namespace_id == ctx.namespace_id)
            .order_by(runs.c.commit_seq.desc())
            .limit(20)
        )
    ).scalars()
    for raw in pending:
        run = MaintenanceRun.model_validate(raw)
        if (
            run.catalog is None
            and run.draft is None
            and run.review is None
            and run.command.references
            and run.command.kind == "growth"
            and run.command.auto_adopt == (policy.mode == "automatic")
        ):
            merged = tuple(
                {ref.exact(): ref for ref in (*run.command.references, *references)}.values()
            )
            if len(merged) > policy.max_snapshot_entries:
                break
            updated = run.model_copy(
                update={"command": run.command.model_copy(update={"references": merged})}
            )
            await tx.connection.execute(
                runs.update()
                .where(runs.c.id == run.id)
                .values(record=updated.model_dump(mode="json"))
            )
            return
    await self.submit_in(
        tx,
        ctx,
        StartMaintenance(
            operation_id="contact-growth:" + observation_id,
            references=references,
            limit=policy.max_snapshot_entries,
            auto_adopt=policy.mode == "automatic",
            instruction=(
                "依据本次主体实际接触和现行认识形成有来源的局部变化。"
                "稳定性格与长期能力需有真实经历支持；单次心情保留情境。"
                "提案保持独立，不把发送成功、沉默或后台执行当作主体理解或他人认可。"
            ),
        ),
    )


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
                    await self.cognition.materials.provenance_in(
                        tx, ctx, source.source, derivations=self.cognition.derivations
                    )
                ).material_ids,
            )
    return GrowthSnapshot(
        watermark=tx.commit_seq,
        entries=entries,
        material_ids=tuple(sorted(set(material_ids))),
        capabilities=current.capabilities,
        sources=material_sources,
    )
