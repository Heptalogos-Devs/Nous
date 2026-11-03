"""Claim directories read canonical heads and the same evidence visibility owner."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CognitiveRecord, CognitiveRef
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.fact_body import ChatFactScope, FactClaimBody, FactScope, PersonFactScope
from nous_sdk.contracts.facts import FactQuery, FactView
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.platform.persistence import ReadView, WriteTx

from .fact_identity import conflicts_with
from .schema import revisions

if TYPE_CHECKING:
    from .records import CognitiveRecords


async def heads_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    scope: FactScope | None = None,
    *,
    entity_id: str | None = None,
) -> tuple[CognitiveRecord, ...]:
    statement = select(revisions.c.record).where(
        revisions.c.namespace_id == ctx.namespace_id,
        revisions.c.kind == "fact_claim",
        revisions.c.head.is_(True),
    )
    if scope is not None:
        target = revisions.c.record["body"]["scope"]
        statement = statement.where(target["kind"].as_string() == scope.kind)
        if isinstance(scope, PersonFactScope):
            statement = statement.where(target["person"]["id"].as_string() == scope.person.id)
        elif isinstance(scope, ChatFactScope):
            statement = statement.where(
                target["conversation_ref"].as_string() == scope.conversation_ref
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
    cognition: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
) -> CognitiveRecord:
    record = await cognition.get_in(view, ctx, reference)
    if record.purged or not isinstance(record.body, FactClaimBody):
        raise DomainError(
            "fact_claim_required", "The exact reference must identify a retained FactClaim"
        )
    return record


async def head_claim_in(
    cognition: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, claim_id: str
) -> CognitiveRecord:
    raw = await view.connection.scalar(
        select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
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
    cognition: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, entity_id: str
) -> tuple[CognitiveRecord, ...]:
    result: list[CognitiveRecord] = []
    for record in await heads_in(view, ctx, entity_id=entity_id):
        body = record.body
        if (
            record.deleted
            or not isinstance(body, FactClaimBody)
            or body.status != "active"
            or (body.stability != "stable")
            or any(item.link.polarity == "counterexample" for item in body.evidence)
            or (
                record.times.validity is not None
                and (not record.times.validity.contains(ctx.clock))
            )
            or (not await cognition.visible_in(view, ctx, record))
        ):
            continue
        result.append(record)
    return tuple(sorted(result, key=lambda record: (record.commit_seq, record.reference.exact())))


async def query_in(
    cognition: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, command: FactQuery
) -> tuple[CognitiveRecord, ...]:
    await require_namespace_in(view, ctx)
    result: list[CognitiveRecord] = []
    for record in await heads_in(view, ctx, command.scope):
        body = record.body
        if (
            record.deleted
            or not isinstance(body, FactClaimBody)
            or body.status not in command.statuses
            or (command.query.casefold() not in (body.fact_key + " " + body.value_text).casefold())
            or (
                command.effective_at is not None
                and record.times.validity is not None
                and (not record.times.validity.contains(command.effective_at))
            )
            or (not await cognition.visible_in(view, ctx, record))
        ):
            continue
        result.append(record)
    result.sort(key=lambda record: (record.commit_seq, record.reference.exact()), reverse=True)
    return tuple(result[: command.limit])


async def view_in(
    cognition: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
) -> FactView:
    record = await claim_in(cognition, view, ctx, reference)
    assert isinstance(record.body, FactClaimBody)
    history: list[CognitiveRecord] = []
    values = (
        await view.connection.execute(
            select(revisions.c.record)
            .where(
                revisions.c.namespace_id == ctx.namespace_id,
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
            and (other.body.status in {"active", "conflicted"})
            and conflicts_with(other.body, other.times.validity, record.body, record.times.validity)
            and await cognition.visible_in(view, ctx, other)
        ):
            conflicts.append(other.reference)
    return FactView(
        record=record,
        history=tuple(history),
        conflicting=tuple(conflicts),
        successors=tuple(successors),
    )
