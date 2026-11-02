"""Evidence-backed platform correspondence and directional social judgments."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    DomainError,
    EntityBody,
    EvidenceLink,
    SocialRelationBody,
)
from nous_sdk.contracts.social import (
    ChangeSocialRelation,
    IdentityHypothesis,
    LinkIdentity,
    UnlinkIdentity,
)
from sqlalchemy import select

from nous.identity.access import require_namespace_in

from .lifecycle import selectable
from .projection import references_in
from .schema import revisions

if TYPE_CHECKING:
    from .records import CognitiveRecords


class SocialService:
    def __init__(self, records: CognitiveRecords):
        self.records = records

    async def resolve_identity(
        self, ctx: CallContext, platform: str, account_id: str
    ) -> CognitiveRecord:
        async with self.records.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "social")
            values = (
                await view.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
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
                    and await self.records.visible_in(view, ctx, record)
                ):
                    matches.append(record)
            if len(matches) != 1:
                raise DomainError(
                    "identity_not_found" if not matches else "identity_ambiguous",
                    "Platform correspondence does not identify one accessible person",
                )
            return matches[0]

    async def link_identity(self, ctx: CallContext, command: LinkIdentity) -> CognitiveRecord:
        async with self.records.authority.write(
            ctx,
            command.operation_id,
            operation_kind="social.link_identity",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.records.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            await require_namespace_in(tx, ctx, "social")
            person = await self.records.get_in(tx, ctx, command.entity)
            if not isinstance(person.body, EntityBody) or person.body.entity_type != "person":
                raise DomainError(
                    "identity_person_required", "Platform correspondence requires a person Entity"
                )
            values = (
                await tx.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
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
            if existing and (not command.replace):
                raise DomainError(
                    "identity_exists",
                    "Updating known platform evidence requires an explicit replacement",
                )
            retained = tuple(
                identity for identity in person.body.platform_identities if identity not in existing
            )
            body = person.body.model_copy(
                update={"platform_identities": (*retained, command.identity)}
            )
            adopted = await self.records.adopt_in(
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
        self, ctx: CallContext, command: ChangeSocialRelation
    ) -> CognitiveRecord:
        async with self.records.authority.write(
            ctx,
            command.operation_id,
            operation_kind="social.relate",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.records.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            if command.target:
                previous = await self.records.get_in(tx, ctx, command.target)
                if not isinstance(previous.body, SocialRelationBody):
                    raise DomainError(
                        "social_relation_target",
                        "Relationship updates must revise a SocialRelation",
                    )
            return tx.result(
                await self.records.adopt_in(
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

    async def social_profile(self, ctx: CallContext, entity_id: str) -> tuple[CognitiveRecord, ...]:
        async with self.records.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "social")
            values = (
                await view.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
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
                    and await self.records.visible_in(view, ctx, record)
                ):
                    result.append(record)
            return tuple(result)

    async def propose_identity(
        self, ctx: CallContext, command: IdentityHypothesis
    ) -> CognitiveRecord:
        from nous_sdk.contracts import AssociationBody, EpistemicStance

        async with self.records.authority.write(
            ctx,
            command.operation_id,
            operation_kind="social.identity_hypothesis",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.records.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            await require_namespace_in(tx, ctx, "social")
            for reference in (command.left, command.right):
                person = await self.records.get_in(tx, ctx, reference)
                if not isinstance(person.body, EntityBody) or person.body.entity_type != "person":
                    raise DomainError(
                        "identity_person_required", "An identity hypothesis requires two people"
                    )
            return tx.result(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=AssociationBody(
                            from_ref=command.left,
                            to_ref=command.right,
                            relation="possible_same_person",
                            basis_class="source_evidence",
                            description=command.explanation,
                            epistemic=EpistemicStance(
                                classification="inferred", confidence=command.confidence
                            ),
                        ),
                        evidence=command.evidence,
                        times=command.times,
                    ),
                )
            )

    async def unlink_identity(self, ctx: CallContext, command: UnlinkIdentity) -> CognitiveRecord:
        async with self.records.authority.write(
            ctx,
            command.operation_id,
            operation_kind="social.unlink_identity",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.records.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            await require_namespace_in(tx, ctx, "social")
            person = await self.records.get_in(tx, ctx, command.entity)
            if not isinstance(person.body, EntityBody):
                raise DomainError(
                    "identity_person_required", "Account correction requires an Entity"
                )
            retained = tuple(
                identity
                for identity in person.body.platform_identities
                if (identity.platform, identity.account_id)
                != (command.platform, command.account_id)
            )
            if retained == person.body.platform_identities:
                raise DomainError(
                    "identity_not_found", "The selected account is not linked to this person"
                )
            return tx.result(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        id=person.reference.id,
                        expected_revision=person.reference.revision,
                        body=person.body.model_copy(update={"platform_identities": retained}),
                        evidence=(
                            *command.evidence,
                            EvidenceLink(
                                reference=person.reference, polarity="boundary", role=command.reason
                            ),
                        ),
                        times=command.times,
                    ),
                )
            )
