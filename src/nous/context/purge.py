"""Context purge owned behavior."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveRef,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.platform.persistence import WriteTx
from nous.platform.persistence.authority import canonical_json

from .models import contexts, preparations, tracks

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def purge_references_in(
    self: ManagedContexts, tx: WriteTx, ctx: CallContext, references: tuple[CognitiveRef, ...]
) -> None:
    """Erase cached source-derived text in the same source purge transaction."""
    from nous.material.service import materials

    retired = {reference.id for reference in references}
    original_rows = (
        await tx.connection.execute(
            select(materials.c.id, materials.c.record).where(
                materials.c.namespace_id == ctx.namespace_id
            )
        )
    ).all()
    purged_materials = {str(row.id) for row in original_rows if row.record.get("purged", False)}

    def scrub(value: JsonValue) -> JsonValue:
        if isinstance(value, list):
            return [scrub(item) for item in value]
        if not isinstance(value, dict):
            return value
        result = {key: scrub(item) for key, item in value.items()}
        targets = value.get("sources", value.get("anchors", []))
        matches = False
        if isinstance(targets, list):
            for target in targets:
                if isinstance(target, dict) and isinstance(
                    reference := target.get("reference"), dict
                ):
                    identity = reference.get("id")
                    if (target.get("kind") == "cognition" and identity in retired) or (
                        target.get("kind") == "media" and identity in purged_materials
                    ):
                        matches = True
        if matches and "text" in result:
            result["text"] = ""
            result["provenance"] = {"purged": True}
        if matches and "context_text" in result:
            result["context_text"] = ""
        return result

    for table in (preparations, tracks, contexts):
        rows = (
            await tx.connection.execute(
                select(table.c.id, table.c.record).where(
                    table.c.namespace_id == ctx.namespace_id,
                    table.c.subject_id == ctx.subject_id,
                )
            )
        ).all()
        for row in rows:
            old = TypeAdapter(JsonValue).validate_python(row.record)
            cleaned = scrub(old)
            if cleaned != old:
                if (
                    table is preparations
                    and isinstance(cleaned, dict)
                    and isinstance(preparation := cleaned.get("preparation"), dict)
                    and preparation.get("state") == "prepared"
                ):
                    preparation["state"] = "cancelled"
                await tx.connection.execute(
                    update(table).where(table.c.id == row.id).values(record=cleaned)
                )
    from nous.platform.persistence.schema import operations

    operation_rows = (
        await tx.connection.execute(
            select(operations.c.operation_key, operations.c.result_json).where(
                operations.c.namespace_id == ctx.namespace_id,
                operations.c.scope == ctx.subject_id,
                operations.c.operation_kind.like("context.%"),
            )
        )
    ).all()
    for row in operation_rows:
        old = TypeAdapter(JsonValue).validate_json(row.result_json)
        cleaned = scrub(old)
        if cleaned != old:
            await tx.connection.execute(
                update(operations)
                .where(operations.c.operation_key == row.operation_key)
                .values(result_json=canonical_json(cleaned))
            )
