"""Retained people keep only attributes with surviving independent support."""

from __future__ import annotations

from nous_sdk.contracts import CognitiveRecord, EntityBody, EvidenceLink
from nous_sdk.contracts.cognition import PlatformIdentity
from pydantic import JsonValue
from sqlalchemy import select, update

from nous.cognition.schema import revisions, vectors
from nous.platform.persistence import WriteTx


async def supported_in(
    tx: WriteTx, record: CognitiveRecord, links: tuple[EvidenceLink, ...], unpurged: set[str]
) -> bool:
    for link in links:
        if link.polarity != "support":
            continue
        if link.observation_id and link.observation_id in unpurged:
            return True
        if link.reference:
            value = await tx.connection.scalar(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == record.namespace_id,
                    revisions.c.subject_id == record.subject_id,
                    revisions.c.id == link.reference.id,
                    revisions.c.revision == link.reference.revision,
                )
            )
            if value is not None:
                source = CognitiveRecord.model_validate(value)
                if not source.purged and set(
                    source.support_roots or source.source_roots
                ).intersection(unpurged):
                    return True
    return False


async def redact_attributes_in(tx: WriteTx, record: CognitiveRecord, unpurged: set[str]) -> bool:
    if not isinstance(record.body, EntityBody):
        return False
    kept = tuple(
        [
            attribute
            for attribute in record.body.attributes
            if await supported_in(tx, record, attribute.evidence, unpurged)
        ]
    )
    identities: list[PlatformIdentity] = []
    for identity in record.body.platform_identities:
        if not await supported_in(tx, record, identity.evidence, unpurged):
            continue
        attributes = tuple(
            [
                attribute
                for attribute in identity.attributes
                if await supported_in(tx, record, attribute.evidence, unpurged)
            ]
        )
        identities.append(identity.model_copy(update={"attributes": attributes}))
    controls = record.body.profile_controls
    aliases = controls.aliases
    summary = controls.summary
    if aliases is not None and not await supported_in(tx, record, aliases.evidence, unpurged):
        aliases = None
    if summary is not None and not await supported_in(tx, record, summary.evidence, unpurged):
        summary = None
    controls = controls.model_copy(update={"aliases": aliases, "summary": summary})
    if (
        len(kept) == len(record.body.attributes)
        and tuple(identities) == record.body.platform_identities
        and controls == record.body.profile_controls
    ):
        return False
    body = record.body.model_copy(
        update={
            "attributes": kept,
            "platform_identities": tuple(identities),
            "profile_controls": controls,
            "purged_identity_count": record.body.purged_identity_count
            + len(record.body.platform_identities)
            - len(identities),
            "purged_attribute_count": record.body.purged_attribute_count
            + len(record.body.attributes)
            - len(kept),
        }
    )
    value = record.model_copy(update={"body": body})
    await tx.connection.execute(
        update(revisions)
        .where(
            revisions.c.id == record.reference.id, revisions.c.revision == record.reference.revision
        )
        .values(record=value.model_dump(mode="json"))
    )
    await tx.connection.execute(
        vectors.delete().where(vectors.c.reference == record.reference.exact())
    )
    return True


def contains_record(value: JsonValue, references: set[str]) -> bool:
    if isinstance(value, list):
        return any(contains_record(child, references) for child in value)
    if not isinstance(value, dict):
        return False
    reference = value.get("reference")
    if "body" in value and isinstance(reference, dict):
        key = f"{reference.get('kind')}:{reference.get('id')}@{reference.get('revision')}"
        if key in references:
            return True
    return any(contains_record(child, references) for child in value.values())
