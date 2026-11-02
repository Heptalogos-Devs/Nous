"""Atomic entity merge/split, current consumers and fact attribution migration."""

from __future__ import annotations

from collections import Counter
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    CognitiveRecord,
    CognitiveRef,
    EntityBody,
    EvidenceLink,
    MergeEntities,
    RevisionCause,
    SplitEntity,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.cognition import PersonAliasOverride, PersonSummaryOverride
from sqlalchemy import select

from nous.cognition.facts import FactService
from nous.platform.persistence import WriteTx

from .fact_reads import heads_in as fact_heads_in
from .reference_changes import replace_references_in
from .lifecycle import selectable
from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from .records import CognitiveRecords


async def entity_in(self, tx: WriteTx, ctx: CallContext, ref: CognitiveRef) -> CognitiveRecord:
    record = await self.get_in(tx, ctx, ref)
    if not isinstance(record.body, EntityBody) or not selectable(record):
        raise DomainError("entity_required", "Identity changes require active Entity revisions")
    head = await tx.connection.scalar(
        select(revisions.c.revision).where(
            revisions.c.id == ref.id,
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.head.is_(True),
        )
    )
    if head != ref.revision:
        raise DomainError("revision_conflict", "The Entity changed since the selected revision")
    return record


class EntityService:
    def __init__(self, records: CognitiveRecords, *, facts: FactService):
        self.facts = facts
        self.records = records

    async def merge_entities(
        self, ctx: CallContext, command: MergeEntities
    ) -> tuple[CognitiveRecord, ...]:
        async with self.records.authority.write(ctx) as tx:
            values = [
                await entity_in(self.records, tx, ctx, ref)
                for ref in (command.target, *command.sources)
            ]
            if len({record.reference.id for record in values}) != len(values):
                raise DomainError("entity_merge", "Merge requires distinct Entity identities")
            bodies = [record.body for record in values if isinstance(record.body, EntityBody)]
            if len({body.entity_type for body in bodies}) != 1:
                raise DomainError("entity_type_conflict", "Merge requires the same Entity kind")
            identifiers: dict[str, str] = {}
            for body in bodies:
                for key, value in body.identifiers.items():
                    if key in identifiers and identifiers[key] != value:
                        raise DomainError(
                            "entity_identifier_conflict",
                            "Reconcile conflicting identifier values before merging",
                            identifier=key,
                        )
                    identifiers[key] = value
            target = values[0]
            assert isinstance(target.body, EntityBody)
            aliases = tuple(
                dict.fromkeys(
                    alias
                    for body in bodies
                    for alias in (body.label, *body.effective_aliases)
                    if alias != target.body.label
                )
            )
            controls = target.body.profile_controls
            support = tuple(
                EvidenceLink(reference=record.reference, role=command.reason) for record in values
            )
            if any(body.profile_controls.aliases is not None for body in bodies):
                controls = controls.model_copy(
                    update={"aliases": PersonAliasOverride(aliases=aliases, evidence=support)}
                )
            if any(body.profile_controls.summary is not None for body in bodies):
                summary = "\n\n".join(
                    dict.fromkeys(body.profile_text for body in bodies if body.profile_text)
                )
                if summary:
                    controls = controls.model_copy(
                        update={"summary": PersonSummaryOverride(text=summary, evidence=support)}
                    )
            merged_body = target.body.model_copy(
                update={
                    "aliases": aliases,
                    "description": "\n\n".join(
                        dict.fromkeys(body.description for body in bodies if body.description)
                    ),
                    "profile_controls": controls,
                    "generated_profile": None,
                    "identifiers": identifiers,
                    "attributes": tuple(
                        dict.fromkeys(item for body in bodies for item in body.attributes)
                    ),
                    "mentions": tuple(
                        dict.fromkeys(item for body in bodies for item in body.mentions)
                    ),
                    "platform_identities": tuple(
                        {
                            (item.platform, item.account_id): item
                            for body in bodies
                            for item in body.platform_identities
                        }.values()
                    ),
                    "lineage": tuple(record.reference for record in values),
                }
            )
            retiring = frozenset(record.reference.id for record in values[1:])
            merged = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=target.reference.id,
                    expected_revision=target.reference.revision,
                    body=merged_body,
                    evidence=tuple(
                        EvidenceLink(reference=record.reference, role=command.reason)
                        for record in values
                    ),
                    times=target.times,
                ),
                retiring_entities=retiring,
                state=RevisionState(
                    cause=RevisionCause(
                        action="entity.merge",
                        actor=ctx.caller_id,
                        reason=command.reason,
                        parent=target.reference,
                    )
                ),
            )
            result = [merged]
            for before in values[1:]:
                assert isinstance(before.body, EntityBody)
                result.append(
                    await self.records.adopt_in(
                        tx,
                        ctx,
                        AdoptCognition(
                            id=before.reference.id,
                            expected_revision=before.reference.revision,
                            body=before.body.model_copy(
                                update={
                                    "state": "merged",
                                    "canonical": merged.reference,
                                    "successors": (merged.reference,),
                                    "lineage": (before.reference,),
                                }
                            ),
                            evidence=(
                                EvidenceLink(reference=before.reference, role=command.reason),
                            ),
                            times=before.times,
                        ),
                        state=RevisionState(
                            cause=RevisionCause(
                                action="entity.merge",
                                actor=ctx.caller_id,
                                reason=command.reason,
                                parent=before.reference,
                            )
                        ),
                    )
                )
            mapping = {record.reference.id: merged.reference for record in values}
            if merged_body.entity_type == "person":
                claims, claim_mapping = await self.facts.reassign_person_claims_in(
                    tx,
                    ctx,
                    tuple(record.reference for record in values[1:]),
                    merged.reference,
                    command.reason,
                )
                result.extend(claims)
                mapping.update(claim_mapping)
            result.extend(
                await replace_references_in(
                    self.records,
                    tx,
                    ctx,
                    mapping,
                    command.reason,
                    exclude=frozenset(record.reference.id for record in values),
                )
            )
            return tuple(result)

    async def split_entity(
        self, ctx: CallContext, command: SplitEntity
    ) -> tuple[CognitiveRecord, ...]:
        async with self.records.authority.write(ctx) as tx:
            original = await entity_in(self.records, tx, ctx, command.source)
            assert isinstance(original.body, EntityBody)
            bodies = [part.body for part in command.partitions]
            if any(
                body.entity_type != original.body.entity_type or body.state != "active"
                for body in bodies
            ):
                raise DomainError(
                    "entity_partition",
                    "Split partitions must retain the Entity kind and active state",
                )
            for field in ("attributes", "mentions", "platform_identities"):
                before = Counter(item.model_dump_json() for item in getattr(original.body, field))
                after = Counter(
                    item.model_dump_json() for body in bodies for item in getattr(body, field)
                )
                if before != after:
                    raise DomainError(
                        "entity_partition",
                        "Assign every sourced Entity field exactly once",
                        field=field,
                    )
            if Counter(original.body.identifiers.items()) != Counter(
                pair for body in bodies for pair in body.identifiers.items()
            ):
                raise DomainError(
                    "entity_partition", "Assign every external identifier exactly once"
                )
            evidence = (EvidenceLink(reference=original.reference, role=command.reason),)
            result: list[CognitiveRecord] = []
            for body in bodies:
                result.append(
                    await self.records.adopt_in(
                        tx,
                        ctx,
                        AdoptCognition(
                            body=body.model_copy(update={"lineage": (original.reference,)}),
                            evidence=evidence,
                            times=original.times,
                        ),
                        retiring_entities=frozenset({original.reference.id}),
                    )
                )
            successors = tuple(record.reference for record in result)
            claim_mapping: dict[str, CognitiveRef] = {}
            if original.body.entity_type == "person":
                from nous_sdk.contracts.fact_body import FactClaimBody, PersonFactScope

                old_claims = {
                    record.reference.exact()
                    for record in await fact_heads_in(
                        tx, ctx, PersonFactScope(person=original.reference)
                    )
                    if not record.deleted
                    and isinstance(record.body, FactClaimBody)
                    and (record.body.status in {"active", "conflicted"})
                }
                assigned = [ref for part in command.partitions for ref in part.claims]
                if {ref.exact() for ref in assigned} != old_claims or len(assigned) != len(
                    {ref.exact() for ref in assigned}
                ):
                    raise DomainError(
                        "entity_claim_assignment", "Assign each current person claim exactly once"
                    )
                if old_claims and self.facts is None:
                    raise DomainError(
                        "fact_owner_unavailable", "Person splitting requires the Fact owner"
                    )
                for part, successor in zip(command.partitions, successors, strict=True):
                    claims, mapping = await self.facts.reassign_person_claims_in(
                        tx,
                        ctx,
                        (original.reference,),
                        successor,
                        command.reason,
                        claims=part.claims,
                    )
                    result.extend(claims)
                    claim_mapping.update(mapping)
            result.append(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        id=original.reference.id,
                        expected_revision=original.reference.revision,
                        body=original.body.model_copy(
                            update={
                                "state": "split",
                                "successors": successors,
                                "lineage": (original.reference,),
                            }
                        ),
                        evidence=evidence,
                        times=original.times,
                    ),
                    state=RevisionState(
                        cause=RevisionCause(
                            action="entity.split",
                            actor=ctx.caller_id,
                            reason=command.reason,
                            parent=original.reference,
                        )
                    ),
                )
            )
            result.extend(
                await replace_references_in(
                    self.records,
                    tx,
                    ctx,
                    {original.reference.id: successors[0]},
                    command.reason,
                    exclude=frozenset({original.reference.id, *(ref.id for ref in successors)}),
                    partitions=successors,
                    assignments=command.consumers,
                )
            )
            if claim_mapping:
                result.extend(
                    await replace_references_in(
                        self.records, tx, ctx, claim_mapping, command.reason
                    )
                )
            return tuple(result)
