"""Erase exact derived revisions and downstream content when any actual input is purged."""

from typing import TYPE_CHECKING

from nous_sdk.contracts.derivation import DerivationQuality, DerivedRegion, DerivedRepresentation
from pydantic import JsonValue
from sqlalchemy import select

from nous.platform.persistence import WriteTx

from .derived_schema import derived_regions, representations
from .service import observations

if TYPE_CHECKING:
    from .derived import DerivedService


def exact_ref(value: JsonValue) -> str | None:
    if isinstance(value, dict) and value.get("kind") is None:
        identity, revision = value.get("id"), value.get("revision")
        if isinstance(identity, str) and isinstance(revision, int):
            return f"representation:{identity}@{revision}"
    return None


async def purge_materials_in(
    self: DerivedService, tx: WriteTx, material_ids: tuple[str, ...]
) -> None:
    purged_contacts = {
        str(row["id"])
        for row in (
            await tx.connection.execute(
                select(observations.c.id, observations.c.record).where(
                    observations.c.namespace_id == tx.ctx.namespace_id,
                )
            )
        ).mappings()
        if row["record"].get("purged")
    }
    records = [
        DerivedRepresentation.model_validate(value)
        for value in (
            await tx.connection.execute(
                select(representations.c.record).where(
                    representations.c.namespace_id == tx.ctx.namespace_id
                )
            )
        ).scalars()
    ]
    regions = [
        DerivedRegion.model_validate(value)
        for value in (
            await tx.connection.execute(
                select(derived_regions.c.record).where(
                    derived_regions.c.namespace_id == tx.ctx.namespace_id
                )
            )
        ).scalars()
    ]
    parents = {region.id: region.representation.media().exact() for region in regions}
    affected: set[str] = set()
    pending = list(records)
    while pending:
        changed = False
        for record in list(pending):
            lost = (
                record.purged
                or bool(set(record.material_ids).intersection(material_ids))
                or bool(set(record.source_roots).intersection(purged_contacts))
            )
            lost = lost or any(
                item.source.observation_id in purged_contacts
                or item.source.kind == "representation"
                and item.source.exact() in affected
                or item.source.kind == "derived_region"
                and parents.get(item.source.id) in affected
                for item in record.inputs
            )
            if not lost:
                continue
            affected.add(record.reference.media().exact())
            pending.remove(record)
            changed = True
            if not record.purged:
                updated = record.model_copy(
                    update={"payload": None, "purged": True, "quality": DerivationQuality()}
                )
                await tx.connection.execute(
                    representations.update()
                    .where(
                        representations.c.namespace_id == record.namespace_id,
                        representations.c.subject_id == record.subject_id,
                        representations.c.id == record.reference.id,
                        representations.c.revision == record.reference.revision,
                    )
                    .values(record=updated.model_dump(mode="json"))
                )
        if not changed:
            break
    affected_regions: set[str] = set()
    for region in regions:
        if parents[region.id] in affected:
            affected_regions.add(region.id)
            if not region.purged:
                await tx.connection.execute(
                    derived_regions.update()
                    .where(derived_regions.c.id == region.id)
                    .values(
                        record=region.model_copy(
                            update={"purged": True, "selector": None}
                        ).model_dump(mode="json")
                    )
                )

    def contains(value: JsonValue) -> bool:
        if isinstance(value, dict):
            kind, identity = value.get("kind"), value.get("id")
            if (
                kind == "representation"
                and isinstance(identity, str)
                and f"representation:{identity}" in affected
            ):
                return True
            if (
                kind == "derived_region"
                and isinstance(identity, str)
                and identity in affected_regions
            ):
                return True
            if (
                exact_ref(value.get("reference")) in affected
                or exact_ref(value.get("representation")) in affected
            ):
                return True
            return any(contains(item) for item in value.values())
        return isinstance(value, list) and any(contains(item) for item in value)

    if affected:
        await self.authority.redact_receipts_in(tx, contains)
