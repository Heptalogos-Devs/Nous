"""Retire unsupported payloads through actual evidence, preserving independent proof."""

from __future__ import annotations

from collections import deque
from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, CognitiveRecord, CognitiveRef, EntityBody
from sqlalchemy import select, update

from nous.material.service import observations
from nous.platform.persistence import WriteTx

from .entities import contains_record, redact_attributes_in
from .projection import media_references_in
from .schema import revisions, vectors

if TYPE_CHECKING:
    from .service import CognitionService


async def retire_unsupported_in(
    self: CognitionService, tx: WriteTx, ctx: CallContext
) -> tuple[CognitiveRef, ...]:
    sources = {
        str(row.id)
        for row in (
            await tx.connection.execute(
                select(observations.c.id, observations.c.record).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.subject_id == ctx.subject_id,
                )
            )
        ).all()
        if not row.record.get("purged")
    }
    values = (
        await tx.connection.execute(
            select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.subject_id == ctx.subject_id,
            )
        )
    ).scalars()
    records = {
        record.reference.exact(): record
        for record in (CognitiveRecord.model_validate(value) for value in values)
    }
    invalid_media: set[str] = set()
    for key, record in records.items():
        for reference in media_references_in(record.body):
            availability = await self.derivations.availability_in(tx, ctx, reference)
            if availability in {"purged", "missing"}:
                invalid_media.add(key)
                break
    supported: set[str] = set()
    dependants: dict[str, set[str]] = {}
    for key, record in records.items():
        if record.purged or key in invalid_media:
            continue
        links = list(record.evidence)
        if isinstance(record.body, EntityBody):
            links.extend(
                link for attribute in record.body.attributes for link in attribute.evidence
            )
            links.extend(
                link for identity in record.body.platform_identities for link in identity.evidence
            )
        if tuple(media_references_in(record.body)) and set(record.support_roots).intersection(
            sources
        ):
            supported.add(key)
        for link in links:
            if link.polarity != "support":
                continue
            if link.observation_id in sources:
                supported.add(key)
            if link.reference:
                dependants.setdefault(link.reference.exact(), set()).add(key)
    # Reachability computes the least supported set, including imported cyclic graphs.
    pending = deque(supported)
    while pending:
        for key in dependants.get(pending.popleft(), ()):
            if key not in supported:
                supported.add(key)
                pending.append(key)

    retired: list[CognitiveRef] = []
    for record in records.values():
        if record.purged or record.reference.exact() in supported:
            continue
        tombstone = record.model_copy(
            update={
                "body": None,
                "deleted": True,
                "purged": True,
                "evidence": tuple(
                    link.model_copy(update={"role": "purged"}) for link in record.evidence
                ),
            }
        )
        await tx.connection.execute(
            update(revisions)
            .where(
                revisions.c.id == record.reference.id,
                revisions.c.revision == record.reference.revision,
            )
            .values(record=tombstone.model_dump(mode="json"))
        )
        await tx.connection.execute(
            vectors.delete().where(vectors.c.reference == record.reference.exact())
        )
        retired.append(record.reference)
    for record in records.values():
        if record.reference.exact() in supported and await redact_attributes_in(
            tx, record, sources
        ):
            retired.append(record.reference)
    if retired:
        keys = {reference.exact() for reference in retired}
        await self.authority.redact_receipts_in(tx, lambda value: contains_record(value, keys))
    return tuple(retired)
