"""Claim directories read canonical heads and the same evidence visibility owner."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, CognitiveRecord, CognitiveRef, DomainError
from nous_wave.contracts.fact_body import FactClaimBody, FactScope, PersonFactScope
from nous_wave.contracts.facts import FactQuery, FactView
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.platform.persistence import ReadView, WriteTx

from .fact_identity import conflict_group
from .schema import revisions

if TYPE_CHECKING:
    from .service import CognitionService


async def heads_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    scope: FactScope | None = None,
    *,
    entity_id: str | None = None,
) -> tuple[CognitiveRecord, ...]:
    statement = select(revisions.c.record).where(
        revisions.c.namespace_id == ctx.namespace_id,
        revisions.c.subject_id == ctx.require_subject(),
        revisions.c.kind == "fact_claim",
        revisions.c.head.is_(True),
    )
    if scope is not None:
        target = revisions.c.record["body"]["scope"]
        statement = statement.where(target["kind"].as_string() == scope.kind)
        statement = statement.where(
            target["person"]["id"].as_string() == scope.person.id
            if isinstance(scope, PersonFactScope)
            else target["conversation_ref"].as_string() == scope.conversation_ref
        )
    if entity_id is not None:
        target = revisions.c.record["body"]["scope"]
        statement = statement.where(
            target["kind"].as_string() == "person", target["person"]["id"].as_string() == entity_id
        )
    return tuple(
        CognitiveRecord.model_validate(value)
        for value in (await view.connection.execute(statement)).scalars()
    )


async def claim_in(
    cognition: CognitionService, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
) -> CognitiveRecord:
    record = await cognition.get_in(view, ctx, reference)
    if record.purged or not isinstance(record.body, FactClaimBody):
        raise DomainError(
            "fact_claim_required", "The exact reference must identify a retained FactClaim"
        )
    return record


async def head_claim_in(
    cognition: CognitionService, view: ReadView | WriteTx, ctx: CallContext, claim_id: str
) -> CognitiveRecord:
    raw = await view.connection.scalar(
        select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.subject_id == ctx.require_subject(),
            revisions.c.kind == "fact_claim",
            revisions.c.id == claim_id,
            revisions.c.head.is_(True),
        )
    )
    if raw is None:
        raise DomainError("fact_claim_not_found", "The claim is unavailable in this scope")
    record = CognitiveRecord.model_validate(raw)
    return await claim_in(cognition, view, ctx, record.reference)


async def current_person_claims_in(
    cognition: CognitionService, view: ReadView | WriteTx, ctx: CallContext, entity_id: str
) -> tuple[CognitiveRecord, ...]:
    result: list[CognitiveRecord] = []
    for record in await heads_in(view, ctx, entity_id=entity_id):
        body = record.body
        if (
            record.deleted
            or not isinstance(body, FactClaimBody)
            or body.status != "active"
            or body.stability != "stable"
            or any(item.link.polarity == "counterexample" for item in body.evidence)
            or record.times.validity is not None
            and not record.times.validity.contains(ctx.clock)
            or not await cognition.visible_in(view, ctx, record)
        ):
            continue
        result.append(record)
    return tuple(sorted(result, key=lambda record: (record.commit_seq, record.reference.exact())))


async def query_in(
    cognition: CognitionService, view: ReadView | WriteTx, ctx: CallContext, command: FactQuery
) -> tuple[CognitiveRecord, ...]:
    await require_subject_in(view, ctx)
    result: list[CognitiveRecord] = []
    for record in await heads_in(view, ctx, command.scope):
        body = record.body
        if (
            record.deleted
            or not isinstance(body, FactClaimBody)
            or body.status not in command.statuses
            or command.query.casefold() not in (body.fact_key + " " + body.value_text).casefold()
            or command.effective_at is not None
            and record.times.validity is not None
            and not record.times.validity.contains(command.effective_at)
            or not await cognition.visible_in(view, ctx, record)
        ):
            continue
        result.append(record)
    result.sort(key=lambda record: (record.commit_seq, record.reference.exact()), reverse=True)
    return tuple(result[: command.limit])


async def view_in(
    cognition: CognitionService, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
) -> FactView:
    record = await claim_in(cognition, view, ctx, reference)
    assert isinstance(record.body, FactClaimBody)
    history: list[CognitiveRecord] = []
    values = (
        await view.connection.execute(
            select(revisions.c.record)
            .where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.subject_id == ctx.subject_id,
                revisions.c.id == reference.id,
                revisions.c.revision <= reference.revision,
            )
            .order_by(revisions.c.revision)
        )
    ).scalars()
    for value in values:
        previous = CognitiveRecord.model_validate(value)
        if await cognition.visible_in(view, ctx, previous):
            history.append(previous)
    conflicts: list[CognitiveRef] = []
    successors: list[CognitiveRef] = []
    group = conflict_group(record.body)
    for other in await heads_in(view, ctx, record.body.scope):
        if (
            isinstance(other.body, FactClaimBody)
            and any(ref.id == reference.id for ref in other.body.supersedes)
            and await cognition.visible_in(view, ctx, other)
        ):
            successors.append(other.reference)
        if (
            other.reference.id != reference.id
            and isinstance(other.body, FactClaimBody)
            and other.body.status == "active"
            and conflict_group(other.body) == group
            and await cognition.visible_in(view, ctx, other)
        ):
            conflicts.append(other.reference)
    return FactView(
        record=record,
        history=tuple(history),
        conflicting=tuple(conflicts),
        successors=tuple(successors),
    )
