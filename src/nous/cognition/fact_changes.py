"""Deterministic claim transitions adapted from A-Memorix's actual fact ledger."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EvidenceLink,
)
from nous_wave.contracts.fact_body import FactClaimBody, FactClaimSpec, FactEvidence, FactStatus
from nous_wave.contracts.facts import FactChange

from nous.platform.persistence import WriteTx

from .fact_identity import conflict_group, identity, normalized, scope_identity

if TYPE_CHECKING:
    from .facts import FactService


def merged_evidence(
    before: tuple[FactEvidence, ...], incoming: tuple[FactEvidence, ...]
) -> tuple[FactEvidence, ...]:
    def key(item: FactEvidence) -> tuple[str | None, CognitiveRef | None, str | None, str]:
        return (
            item.link.observation_id,
            item.link.reference,
            item.link.region_id,
            item.link.polarity,
        )

    result = list(before)
    for item in incoming:
        index = next((index for index, old in enumerate(result) if key(old) == key(item)), None)
        if index is None:
            result.append(item)
        else:
            weights = [value for value in (result[index].weight, item.weight) if value is not None]
            result[index] = item.model_copy(update={"weight": max(weights) if weights else None})
    return tuple(result)


async def apply_in(
    self: FactService,
    tx: WriteTx,
    ctx: CallContext,
    command: FactChange,
    spec: FactClaimSpec,
    requested: CognitiveRecord | None,
    candidates: tuple[CognitiveRecord, ...],
    explicit_supersedes: tuple[CognitiveRecord, ...],
    report: EvidenceLink,
) -> tuple[CognitiveRecord, tuple[CognitiveRecord, ...], tuple[CognitiveRef, ...]]:
    operation = command.action
    if operation == "update":
        if requested is None or not isinstance(requested.body, FactClaimBody):
            raise DomainError("fact_reference_required", "Updating requires an existing claim")
        operation = "classify" if identity(requested.body) == identity(spec) else "assert"
    if requested is not None:
        assert isinstance(requested.body, FactClaimBody)
        if scope_identity(requested.body.scope) != scope_identity(spec.scope):
            raise DomainError(
                "fact_scope_immutable", "A claim update cannot change its person/chat scope"
            )
    target = next(
        (
            record
            for record in candidates
            if isinstance(record.body, FactClaimBody) and identity(record.body) == identity(spec)
        ),
        None,
    )
    if target is not None:
        assert isinstance(target.body, FactClaimBody)
        if target.body.cardinality != spec.cardinality:
            raise DomainError(
                "fact_cardinality_immutable", "An existing claim cannot change cardinality"
            )
    group = conflict_group(spec)
    supersedes = list(explicit_supersedes)
    changed: list[CognitiveRecord] = []
    if requested is not None and (target is None or target.reference.id != requested.reference.id):
        if operation != "assert":
            raise DomainError(
                "fact_identity_immutable", "Classification cannot change the claim identity"
            )
        assert isinstance(requested.body, FactClaimBody)
        if conflict_group(requested.body) == group:
            supersedes.append(requested)
        else:
            changed.append(
                await self.revise_in(
                    tx,
                    ctx,
                    requested.body.model_copy(update={"status": "retracted"}),
                    requested,
                    report,
                    "retract",
                    command,
                    apply_requested_validity=False,
                )
            )
    for previous in supersedes:
        assert isinstance(previous.body, FactClaimBody)
        if conflict_group(previous.body) != group:
            raise DomainError(
                "fact_conflict_group", "Supersession requires the same exact conflict group"
            )
    retired_ids = {record.reference.id for record in (*changed, *supersedes)}
    conflicts = tuple(
        record.reference
        for record in candidates
        if isinstance(record.body, FactClaimBody)
        and record.body.status == "active"
        and conflict_group(record.body) == group
        and record.reference.id not in retired_ids
        and (target is None or record.reference.id != target.reference.id)
    )
    if operation != "assert" and requested is None:
        raise DomainError(
            "fact_reference_required", "A transition requires the exact current claim"
        )
    prior = target.body if target is not None else None
    assert prior is None or isinstance(prior, FactClaimBody)
    evidence = merged_evidence(prior.evidence if prior else (), command.evidence)
    status: FactStatus = "conflicted" if conflicts else "active"
    action = "reinforce" if target else "assert"
    if operation in {"classify", "add_evidence"}:
        assert prior is not None
        status, action = prior.status, operation
        if operation == "add_evidence" and not command.evidence:
            raise DomainError(
                "fact_evidence_required", "Adding evidence requires its actual source"
            )
    elif operation == "retract":
        status, action = "retracted", "retract"
    elif operation == "restore":
        action = "restore"
    values = spec.model_dump()
    values["fact_key"] = normalized(spec.fact_key)
    body = FactClaimBody(
        **values,
        status=status,
        evidence=evidence,
        supersedes=tuple(
            dict.fromkeys(
                (*(prior.supersedes if prior else ()), *(record.reference for record in supersedes))
            )
        ),
        first_observed=prior.first_observed if prior else None,
        last_confirmed=prior.last_confirmed if prior else None,
    )
    context = (
        (EvidenceLink(reference=requested.reference, polarity="boundary", role="fact.revision"),)
        if requested is not None
        and (target is None or requested.reference.id != target.reference.id)
        else ()
    )
    value = await self.revise_in(
        tx, ctx, body, target, report, action, command, extra_evidence=context
    )
    for previous in supersedes:
        assert isinstance(previous.body, FactClaimBody)
        if previous.body.status == "superseded":
            continue
        changed.append(
            await self.revise_in(
                tx,
                ctx,
                previous.body.model_copy(update={"status": "superseded"}),
                previous,
                report,
                "supersede",
                command,
                apply_requested_validity=False,
                extra_evidence=(
                    EvidenceLink(
                        reference=value.reference, role="fact.superseded_by", polarity="boundary"
                    ),
                ),
            )
        )
    return value, tuple(changed), conflicts
