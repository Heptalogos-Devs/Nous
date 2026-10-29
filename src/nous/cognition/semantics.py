"""Domain-specific references are checked at the common adoption boundary."""

from __future__ import annotations

import math
from collections.abc import Mapping
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveBody,
    CognitiveRecord,
    DomainError,
    EntityBody,
    EpisodeBody,
    JournalBody,
    LanguageConventionBody,
    MemorySemanticsBody,
    SchemaBody,
    SocialRelationBody,
)
from sqlalchemy import select

from nous.platform.persistence import WriteTx

from .projection import references_in
from .schema import revisions

if TYPE_CHECKING:
    from .service import CognitionService


async def validate_domain_in(
    self: CognitionService, tx: WriteTx, ctx: CallContext, body: CognitiveBody, object_id: str
) -> None:
    if isinstance(body, EntityBody) and body.platform_identities:
        own = {(identity.platform, identity.account_id) for identity in body.platform_identities}
        if len(own) != len(body.platform_identities):
            raise DomainError(
                "identity_conflict", "A person cannot contain duplicate platform identities"
            )
        rows = (
            await tx.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.kind == "entity",
                    revisions.c.head.is_(True),
                    revisions.c.id != object_id,
                )
            )
        ).scalars()
        for value in rows:
            record = CognitiveRecord.model_validate(value)
            if (
                isinstance(record.body, EntityBody)
                and not record.deleted
                and own.intersection(
                    (identity.platform, identity.account_id)
                    for identity in record.body.platform_identities
                )
            ):
                raise DomainError(
                    "identity_conflict",
                    "A platform account already belongs to another person in this subject",
                )
    related = {
        reference.exact(): (await self.get_in(tx, ctx, reference)).body
        for reference in references_in(body)
    }
    validate_body(body, object_id, related)


def validate_body(
    body: CognitiveBody, object_id: str, related: Mapping[str, CognitiveBody | None]
) -> None:
    if isinstance(body, MemorySemanticsBody) and body.memory.kind != "memory":
        raise DomainError(
            "memory_semantics_anchor", "Memory semantics require an actual Memory revision"
        )
    if isinstance(body, JournalBody) and any(
        reference.kind != "episode" for reference in body.episodes
    ):
        raise DomainError(
            "journal_episode_required", "Journal episode references must identify Episodes"
        )
    if isinstance(body, SchemaBody) and not body.support:
        raise DomainError("support_required", "Schema requires supporting cognition")
    if isinstance(body, EpisodeBody):
        ordinals = [member.ordinal for member in body.members]
        if len(ordinals) != len(set(ordinals)):
            raise DomainError("episode_order", "Episode member ordinals must be unique")
    if isinstance(body, SocialRelationBody):
        for reference in (body.from_entity, body.to_entity):
            source = related.get(reference.exact())
            if not isinstance(source, EntityBody):
                raise DomainError(
                    "social_entity_required", "Relationship endpoints must name actual Entities"
                )
        for reference in body.groups:
            group = related.get(reference.exact())
            if not isinstance(group, EntityBody) or group.entity_type != "group":
                raise DomainError(
                    "social_group_required",
                    "A relationship group scope must name an actual group Entity",
                )
        if any(
            not math.isfinite(value) or not -1 <= value <= 1
            for value in body.trust_domains.values()
        ):
            raise DomainError(
                "social_trust_invalid", "Trust domains require finite degrees between -1 and 1"
            )
    elif isinstance(body, LanguageConventionBody):
        if not body.audience:
            raise DomainError(
                "language_audience_required", "A language convention requires an actual audience"
            )
        for reference in body.audience:
            source = related.get(reference.exact())
            if not isinstance(source, EntityBody) or source.entity_type not in {
                "person",
                "group",
                "organization",
            }:
                raise DomainError(
                    "language_audience_invalid",
                    "Convention audiences must name actual people or groups",
                )
    elif isinstance(body, EpisodeBody) and body.parent:
        if body.parent.id == object_id:
            raise DomainError(
                "episode_parent_invalid", "An Episode cannot be its own parent identity"
            )
        parent = related.get(body.parent.exact())
        if not isinstance(parent, EpisodeBody) or parent.track != body.track:
            raise DomainError(
                "episode_parent_invalid",
                "Episode parents must belong to the same organizational track",
            )


def capability_for(body: CognitiveBody) -> str:
    if isinstance(body, EntityBody) and body.platform_identities:
        return "social"
    if body.kind in {"social_relation", "language_convention"}:
        return "social"
    if body.kind in {"desire", "concern"}:
        return "motivation"
    if body.kind in {"self_facet", "narrative_thread", "domain_knowledge"}:
        return "self"
    return "memory"
