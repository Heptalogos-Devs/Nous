"""Permanent source erasure fences model admission at the same authority writer."""

from collections.abc import Mapping

from nous_wave.contracts import CallContext
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx


def source_identities(request: Mapping[str, object]) -> set[str]:
    value = TypeAdapter(dict[str, JsonValue]).validate_python(request)
    sources: set[str] = set()
    direct = value.get("source_ids")
    if isinstance(direct, list):
        sources.update(item for item in direct if isinstance(item, str))
    prompt = value.get("prompt")
    variables = prompt.get("variables") if isinstance(prompt, dict) else None
    source = variables.get("source") if isinstance(variables, dict) else None
    if isinstance(source, dict):
        for key in ("material_id", "observation_id"):
            item = source.get(key)
            if isinstance(item, str):
                sources.add(item)
        material_ids = source.get("source_material_ids")
        if isinstance(material_ids, list):
            sources.update(item for item in material_ids if isinstance(item, str))
    parts = value.get("parts")
    if isinstance(parts, list):
        for part in parts:
            reference = part.get("reference") if isinstance(part, dict) else None
            observation_id = (
                reference.get("observation_id") if isinstance(reference, dict) else None
            )
            if isinstance(observation_id, str):
                sources.add(observation_id)
    return sources


async def erased_sources_in(
    view: ReadView | WriteTx, ctx: CallContext, request: Mapping[str, object]
) -> bool:
    from nous.material.service import materials, observations

    identities = source_identities(request)
    if not identities:
        return False
    material_records = (
        await view.connection.execute(
            select(materials.c.record).where(
                materials.c.namespace_id == ctx.namespace_id,
                materials.c.id.in_(identities),
            )
        )
    ).scalars()
    if any(record.get("purged") for record in material_records):
        return True
    records = (
        await view.connection.execute(
            select(observations.c.record).where(
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.subject_id == ctx.subject_id,
                observations.c.id.in_(identities),
            )
        )
    ).scalars()
    return any(record.get("purged") for record in records)
