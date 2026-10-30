"""Evidence-backed platform correspondence and directional social judgments."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    DomainError,
    EntityBody,
    EvidenceLink,
    SocialRelationBody,
)
from nous_wave.contracts.social import ChangeSocialRelation, LinkIdentity
from sqlalchemy import select

from nous.identity.service import require_subject_in

from .lifecycle import selectable
from .projection import references_in
from .schema import revisions

if TYPE_CHECKING:
    from .service import CognitionService


async def resolve_identity(
    self: CognitionService, ctx: CallContext, platform: str, account_id: str
) -> CognitiveRecord:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx, "social")
        values = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.kind == "entity",
                    revisions.c.head.is_(True),
                )
            )
        ).scalars()
        matches: list[CognitiveRecord] = []
        for value in values:
            record = CognitiveRecord.model_validate(value)
            if (
                isinstance(record.body, EntityBody)
                and selectable(record)
                and any(
                    identity.platform == platform and identity.account_id == account_id
                    for identity in record.body.platform_identities
                )
                and await self.visible_in(view, ctx, record)
            ):
                matches.append(record)
        if len(matches) != 1:
            raise DomainError(
                "identity_not_found" if not matches else "identity_ambiguous",
                "Platform correspondence does not identify one accessible person",
            )
        return matches[0]


async def link_identity(
    self: CognitionService, ctx: CallContext, command: LinkIdentity
) -> CognitiveRecord:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="social.link_identity",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await self.replay_record_in(
                tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
            )
        await require_subject_in(tx, ctx, "social")
        person = await self.get_in(tx, ctx, command.entity)
        if not isinstance(person.body, EntityBody) or person.body.entity_type != "person":
            raise DomainError(
                "identity_person_required", "Platform correspondence requires a person Entity"
            )
        values = (
            await tx.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.kind == "entity",
                    revisions.c.head.is_(True),
                )
            )
        ).scalars()
        for value in values:
            record = CognitiveRecord.model_validate(value)
            if (
                record.reference.id != person.reference.id
                and isinstance(record.body, EntityBody)
                and selectable(record)
                and any(
                    identity.platform == command.identity.platform
                    and identity.account_id == command.identity.account_id
                    for identity in record.body.platform_identities
                )
            ):
                raise DomainError(
                    "identity_conflict",
                    "This platform account is already assigned to another person in this subject",
                )
        existing = [
            identity
            for identity in person.body.platform_identities
            if identity.platform == command.identity.platform
            and identity.account_id == command.identity.account_id
        ]
        if existing and not command.replace:
            raise DomainError(
                "identity_exists",
                "Updating known platform evidence requires an explicit replacement",
            )
        retained = tuple(
            identity for identity in person.body.platform_identities if identity not in existing
        )
        body = person.body.model_copy(update={"platform_identities": (*retained, command.identity)})
        adopted = await self.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=person.reference.id,
                expected_revision=person.reference.revision,
                body=body,
                evidence=(*command.identity.evidence, EvidenceLink(reference=person.reference)),
                times=command.times,
            ),
        )
        return tx.result(adopted)


async def change_social_relation(
    self: CognitionService, ctx: CallContext, command: ChangeSocialRelation
) -> CognitiveRecord:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="social.relate",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await self.replay_record_in(
                tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
            )
        if command.target:
            previous = await self.get_in(tx, ctx, command.target)
            if not isinstance(previous.body, SocialRelationBody):
                raise DomainError(
                    "social_relation_target", "Relationship updates must revise a SocialRelation"
                )
        return tx.result(
            await self.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=command.target.id if command.target else None,
                    expected_revision=command.target.revision if command.target else 0,
                    body=command.body,
                    evidence=command.evidence,
                    times=command.times,
                ),
            )
        )


async def social_profile(
    self: CognitionService, ctx: CallContext, entity_id: str
) -> tuple[CognitiveRecord, ...]:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx, "social")
        values = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.head.is_(True),
                    revisions.c.kind.in_(("entity", "social_relation", "language_convention")),
                )
            )
        ).scalars()
        result: list[CognitiveRecord] = []
        for value in values:
            record = CognitiveRecord.model_validate(value)
            references = tuple(ref.id for ref in references_in(record.body))
            if (
                (record.reference.id == entity_id or entity_id in references)
                and selectable(record)
                and await self.visible_in(view, ctx, record)
            ):
                result.append(record)
        return tuple(result)
