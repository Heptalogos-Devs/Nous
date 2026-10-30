"""Exact explicitly captured source names, shared by selection and querying."""

from __future__ import annotations

from collections.abc import Mapping

from nous_sdk.contracts import CallContext
from pydantic import JsonValue
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx


def source_name(
    record: Mapping[str, JsonValue], material: Mapping[str, JsonValue] | None = None
) -> str | None:
    value = record.get("source")
    if isinstance(value, str) and value:
        return value
    value = material.get("source") if material else None
    return value if isinstance(value, str) and value else None


async def named_source_observations_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    names: tuple[str, ...],
    *,
    authorized_observation_ids: frozenset[str] | None = None,
) -> frozenset[str]:
    from .service import materials, observations, source_permitted

    # Query callers pass roots already authorized by the region-aware visibility owner.
    # This whitelist narrows metadata matching; it creates no additional SourceRead grant.
    ctx.require_subject()
    rows = (
        await view.connection.execute(
            select(
                observations.c.id,
                observations.c.material_id,
                observations.c.record,
                materials.c.record.label("material_record"),
            )
            .join(materials, materials.c.id == observations.c.material_id)
            .where(
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.subject_id == ctx.subject_id,
                materials.c.namespace_id == ctx.namespace_id,
            )
        )
    ).mappings()
    return frozenset(
        str(row["id"])
        for row in rows
        if source_name(row["record"], row["material_record"]) in names
        and (
            str(row["id"]) in authorized_observation_ids
            if authorized_observation_ids is not None
            else source_permitted(ctx, str(row["material_id"]), str(row["id"]))
        )
    )
