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

from .models import contexts, feedback, preparations, sessions, tracks

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def purge_references_in(
    self: ManagedContexts, tx: WriteTx, ctx: CallContext, references: tuple[CognitiveRef, ...]
) -> None:
    """Erase cached source-derived text in the same source purge transaction."""
    from nous.material.derived_schema import derived_regions, representations
    from nous.material.service import materials, observations, regions

    retired = {reference.id for reference in references}
    purged_observations = {
        str(row.id)
        for row in (
            await tx.connection.execute(
                select(observations.c.id, observations.c.record).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.subject_id == ctx.subject_id,
                )
            )
        ).all()
        if row.record.get("purged", False)
    }
    purged_media: set[str] = set()
    for table in (materials, regions, derived_regions):
        rows = (
            await tx.connection.execute(
                select(table).where(table.c.namespace_id == ctx.namespace_id)
            )
        ).mappings()
        for row in rows:
            if row["record"].get("purged", False):
                purged_media.add(str(row["id"]))
            elif table is regions and row["material_id"] in purged_media:
                purged_media.add(str(row["id"]))
    representation_rows = (
        await tx.connection.execute(
            select(representations).where(representations.c.namespace_id == ctx.namespace_id)
        )
    ).mappings()
    for row in representation_rows:
        if row["record"].get("purged", False) or purged_observations.intersection(
            row["record"].get("source_roots", [])
        ):
            purged_media.add(f"{row['id']}@{row['revision']}")

    def target_matches(target: JsonValue) -> bool:
        if not isinstance(target, dict):
            return False
        reference = target.get("reference")
        if isinstance(reference, dict):
            identity = reference.get("id")
            if not isinstance(identity, str):
                return False
            return (target.get("kind") == "cognition" and identity in retired) or (
                target.get("kind") == "media"
                and (
                    identity in purged_media
                    or (
                        isinstance(reference.get("observation_id"), str)
                        and reference.get("observation_id") in purged_observations
                    )
                )
            )
        identity = target.get("id")
        return (
            isinstance(identity, str)
            and identity in retired
            and isinstance(target.get("kind"), str)
        )

    def scrub(value: JsonValue) -> JsonValue:
        if isinstance(value, list):
            return [scrub(item) for item in value]
        if not isinstance(value, dict):
            return value
        result = {key: scrub(item) for key, item in value.items()}
        selectors = [
            value.get(key, [])
            for key in ("sources", "anchors", "dependencies", "current_sources", "references")
        ]
        matches = any(
            target_matches(target)
            for targets in selectors
            if isinstance(targets, list)
            for target in targets
        )
        matches = (
            matches or target_matches(value.get("target")) or target_matches(value.get("reference"))
        )
        if matches and "text" in result:
            result["text"] = ""
            result["provenance"] = {"purged": True}
        if matches and "context_text" in result:
            result.update(
                {
                    "purpose": "Source purged",
                    "context_text": "",
                    "unresolved_questions": [],
                    "constraints": None,
                    "resume_conditions": [],
                    "budget_summary": None,
                }
            )
        if matches and "entry_reason" in result:
            result["entry_reason"] = "Source purged"
            if "state" in result:
                result["state"] = "evicted"
        if "work_context" in result and result != value:
            work = result["work_context"]
            if isinstance(work, dict):
                work["purged"] = True
                if work.get("state") == "open":
                    work["state"] = "suspended"
        if matches and "description" in result:
            result["description"] = ""
        if matches and isinstance(usage := result.get("usage"), dict):
            usage["intent"] = ""
        if "preparation" in result and result != value:
            prepared = result["preparation"]
            if isinstance(prepared, dict):
                request = prepared.get("request")
                if isinstance(request, dict) and isinstance(usage := request.get("usage"), dict):
                    usage["intent"] = ""
        return result

    for table in (preparations, tracks, contexts, sessions):
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
                if table is sessions and isinstance(cleaned, dict):
                    previous_revision = cleaned.get("revision", 1)
                    assert isinstance(previous_revision, int)
                    cleaned.update(
                        {
                            "revision": previous_revision + 1,
                            "commit_seq": tx.commit_seq,
                            "history_invalidated": True,
                        }
                    )
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
    feedback_rows = (
        await tx.connection.execute(
            select(feedback.c.id, feedback.c.record)
            .join(contexts, contexts.c.id == feedback.c.context_id)
            .where(
                contexts.c.namespace_id == ctx.namespace_id, contexts.c.subject_id == ctx.subject_id
            )
        )
    ).all()
    for row in feedback_rows:
        old = TypeAdapter(JsonValue).validate_python(row.record)
        cleaned = scrub(old)
        if old != cleaned:
            await tx.connection.execute(
                update(feedback).where(feedback.c.id == row.id).values(record=cleaned)
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
