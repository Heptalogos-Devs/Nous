"""Remove output bytes when their exact source dependency has been purged."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext
from pydantic import JsonValue
from sqlalchemy import select, update

from nous.cognition.schema import revisions, vectors
from nous.material.service import materials, observations, regions
from nous.platform.persistence import WriteTx

if TYPE_CHECKING:
    from .sources import SourceService


async def lost_in(tx: WriteTx, namespace_id: str, locator: str) -> bool:
    if ":" in locator:
        kind, identity = locator.split(":", 1)
        if "@" in identity:
            object_id, revision = identity.rsplit("@", 1)
            if revision.isdigit():
                value = await tx.connection.scalar(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == namespace_id,
                        revisions.c.kind == kind,
                        revisions.c.id == object_id,
                        revisions.c.revision == int(revision),
                    )
                )
                return isinstance(value, dict) and bool(value.get("purged"))
        locator = identity
    value: JsonValue = await tx.connection.scalar(
        select(materials.c.record).where(
            materials.c.id == locator, materials.c.namespace_id == namespace_id
        )
    )
    if isinstance(value, dict):
        return bool(value.get("purged"))
    region_material = await tx.connection.scalar(
        select(regions.c.material_id).where(
            regions.c.id == locator, regions.c.namespace_id == namespace_id
        )
    )
    if region_material is not None:
        return await lost_in(tx, namespace_id, str(region_material))
    value = await tx.connection.scalar(
        select(observations.c.record).where(
            observations.c.id == locator, observations.c.namespace_id == namespace_id
        )
    )
    return isinstance(value, dict) and bool(value.get("purged"))


async def purge_artifacts_in(self: SourceService, tx: WriteTx, ctx: CallContext) -> list[str]:
    detached: list[str] = []
    while True:
        changed = False
        rows = (
            await tx.connection.execute(
                select(materials.c.id, materials.c.record).where(
                    materials.c.namespace_id == ctx.namespace_id
                )
            )
        ).all()
        for material_id, value in rows:
            required: list[str] = value.get("required_source_ids", [])
            if value.get("purged") or not required:
                continue
            if not any([await lost_in(tx, ctx.namespace_id, locator) for locator in required]):
                continue
            await tx.connection.execute(
                update(materials)
                .where(materials.c.id == material_id)
                .values(record={**value, "text": None, "purged": True})
            )
            sources = (
                await tx.connection.execute(
                    select(observations.c.id, observations.c.record).where(
                        observations.c.material_id == material_id
                    )
                )
            ).all()
            for observation_id, record in sources:
                await tx.connection.execute(
                    update(observations)
                    .where(observations.c.id == observation_id)
                    .values(active=False, record={**record, "purged": True})
                )
            await tx.connection.execute(
                vectors.delete().where(vectors.c.reference == f"material:{material_id}")
            )
            if self.content_store:
                detached.extend(await self.content_store.detach_in(tx, str(material_id)))
            if self.purge_in:
                await self.purge_in(tx, (str(material_id),))
            changed = True
        if not changed:
            return detached
