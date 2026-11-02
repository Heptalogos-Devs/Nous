"""Entity attributes and profile projections share their exact evidence rules."""

from __future__ import annotations

from typing import TYPE_CHECKING
from nous_sdk.contracts import CallContext, CognitiveRecord, EntityBody
from sqlalchemy import select, update
from nous.platform.persistence import ReadView, WriteTx
from .projection import evidence_in
from .schema import revisions, vectors

if TYPE_CHECKING:
    from .records import CognitiveRecords


async def project_entity_in(
    cognition: CognitiveRecords,
    view: ReadView | WriteTx,
    ctx: CallContext,
    record: CognitiveRecord,
    *,
    roots: dict[str, str] | None = None,
    permanent: bool = False,
) -> CognitiveRecord:
    if not isinstance(record.body, EntityBody):
        return record
    body = record.body
    roots = roots if roots is not None else await cognition.source_roots_in(view, ctx)
    base_links = await cognition.live_links_in(view, ctx, record.evidence, roots)
    base_live = bool(base_links)
    attributes = tuple(
        [
            attribute
            for attribute in body.attributes
            if await cognition.links_live_in(view, ctx, attribute.evidence, roots)
        ]
    )
    identities = []
    for identity in body.platform_identities:
        if not await cognition.links_live_in(view, ctx, identity.evidence, roots):
            continue
        kept = tuple(
            [
                attribute
                for attribute in identity.attributes
                if await cognition.links_live_in(view, ctx, attribute.evidence, roots)
            ]
        )
        identities.append(identity.model_copy(update={"attributes": kept}))
    controls = body.profile_controls
    if controls.aliases is not None and (
        not await cognition.links_live_in(view, ctx, controls.aliases.evidence, roots)
    ):
        controls = controls.model_copy(update={"aliases": None})
    if controls.summary is not None and (
        not await cognition.links_live_in(view, ctx, controls.summary.evidence, roots)
    ):
        controls = controls.model_copy(update={"summary": None})
    generated = body.generated_profile
    if generated is not None and (
        not await cognition.links_live_in(view, ctx, generated.evidence, roots, require_all=True)
    ):
        generated = None
    body = body.model_copy(
        update={
            "description": body.description if base_live else "",
            "aliases": body.aliases if base_live else (),
            "attributes": attributes,
            "platform_identities": tuple(identities),
            "purged_identity_count": body.purged_identity_count
            + (len(body.platform_identities) - len(identities) if permanent else 0),
            "purged_attribute_count": body.purged_attribute_count
            + (len(body.attributes) - len(attributes) if permanent else 0),
            "profile_controls": controls,
            "generated_profile": generated,
        }
    )
    supported: set[str] = set()
    for link in (*base_links, *tuple(evidence_in(body))):
        if link.polarity != "support":
            continue
        if link.observation_id is not None and link.observation_id in roots:
            supported.add(link.observation_id)
        elif link.reference is not None:
            raw = await view.connection.scalar(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.id == link.reference.id,
                    revisions.c.revision == link.reference.revision,
                )
            )
            if raw is not None:
                source = CognitiveRecord.model_validate(raw)
                supported.update(root for root in source.source_roots if root in roots)
    return record.model_copy(
        update={
            "body": body,
            "evidence": base_links,
            "source_roots": tuple(sorted(supported)),
            "support_roots": tuple(sorted(supported)),
            "source_regions": {
                root: selectors
                for root, selectors in record.source_regions.items()
                if root in supported
            },
        }
    )


async def redact_attributes_in(
    cognition: CognitiveRecords,
    tx: WriteTx,
    ctx: CallContext,
    record: CognitiveRecord,
    unpurged: set[str],
) -> bool:
    if not isinstance(record.body, EntityBody):
        return False
    roots = await cognition.source_roots_in(tx, ctx, purpose="management")
    value = await project_entity_in(
        cognition,
        tx,
        ctx,
        record,
        roots={key: material for key, material in roots.items() if key in unpurged},
        permanent=True,
    )
    if value == record:
        return False
    await tx.connection.execute(
        update(revisions)
        .where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.id == record.reference.id,
            revisions.c.revision == record.reference.revision,
        )
        .values(record=value.model_dump(mode="json"))
    )
    await tx.connection.execute(
        vectors.delete().where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference == record.reference.exact())
    )
    return True
