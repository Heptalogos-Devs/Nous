"""Freeze one Person's exact evidence and adopt its actual paid summary once."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError, EntityBody, EvidenceLink
from nous_sdk.contracts.person_profiles import (
    PersonProfileDraft,
    PersonProfileEvidence,
    PersonProfileModelInput,
    PersonProfileRefreshIntent,
    PersonProfileRefreshSubmission,
    RefreshPersonProfile,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.material.service import require_observation_in
from nous.platform.execution import JobSpec
from nous.platform.persistence import ReadView, WriteTx

from .schema import revisions

if TYPE_CHECKING:
    from .person_profiles import PersonProfileService


async def evidence_sources(
    self: PersonProfileService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    links: tuple[EvidenceLink, ...],
) -> tuple[str, ...]:
    roots: set[str] = set()
    for link in links:
        if link.reference:
            source = await self.cognition.get_in(view, ctx, link.reference)
            if source.deleted or source.purged:
                raise DomainError(
                    "profile_evidence_unavailable", "Profile evidence has been withdrawn"
                )
            roots.update(source.source_roots)
        elif link.observation_id:
            await require_observation_in(view, ctx, link.observation_id, region_id=link.region_id)
            roots.add(link.observation_id)
    return tuple(sorted(roots))


async def submit_refresh(
    self: PersonProfileService,
    ctx: CallContext,
    command: RefreshPersonProfile,
) -> PersonProfileRefreshSubmission:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="person_profile.refresh",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            result = PersonProfileRefreshSubmission.model_validate(tx.replay_result)
            await self.person_in(tx, ctx, result.reference)
            return result
        return tx.result(await submit_refresh_in(self, tx, ctx, command))


async def submit_refresh_in(
    self: PersonProfileService,
    tx: WriteTx,
    ctx: CallContext,
    command: RefreshPersonProfile,
) -> PersonProfileRefreshSubmission:
    from .fact_reads import current_person_claims_in

    await require_subject_in(tx, ctx, "memory")
    record = await self.person_in(tx, ctx, command.reference)
    assert isinstance(record.body, EntityBody)
    indices: list[int] = []
    for index, attribute in enumerate(record.body.attributes):
        try:
            await evidence_sources(self, tx, ctx, attribute.evidence)
        except DomainError as error:
            if error.code not in {
                "source_unavailable",
                "cognition_not_found",
                "region_not_found",
                "profile_evidence_unavailable",
            }:
                raise
            continue
        indices.append(index)
        if len(indices) == command.evidence_limit:
            break
    claims = await current_person_claims_in(self.cognition, tx, ctx, record.reference.id)
    intent = PersonProfileRefreshIntent(
        context=ctx,
        command=command,
        attribute_indices=tuple(indices),
        claim_refs=tuple(
            claim.reference for claim in claims[: max(0, command.evidence_limit - len(indices))]
        ),
    )
    job = await tx.enqueue(
        JobSpec(
            "nous.person_profile.refresh",
            "formation",
            args=(intent.model_dump(mode="json"),),
        )
    )
    return PersonProfileRefreshSubmission(job_id=job.workflow_id, reference=record.reference)


async def profile_input_in(
    self: PersonProfileService,
    view: ReadView | WriteTx,
    intent: PersonProfileRefreshIntent,
) -> tuple[PersonProfileModelInput, dict[str, tuple[EvidenceLink, ...]], tuple[str, ...]]:
    from nous_sdk.contracts.fact_body import FactClaimBody, PersonFactScope

    from nous.platform.persistence.authority import canonical_json

    from .fact_reads import claim_in

    ctx, command = intent.context, intent.command
    record = await self.person_in(view, ctx, command.reference)
    assert isinstance(record.body, EntityBody)
    head = await view.connection.scalar(
        select(revisions.c.head).where(
            revisions.c.id == record.reference.id,
            revisions.c.revision == record.reference.revision,
        )
    )
    if not head:
        raise DomainError("revision_conflict", "The Person changed after its refresh was frozen")
    catalog: dict[str, tuple[EvidenceLink, ...]] = {}
    evidence: list[PersonProfileEvidence] = []
    for index, position in enumerate(intent.attribute_indices):
        attribute = record.body.attributes[position]
        key = f"E{index + 1}"
        catalog[key] = attribute.evidence
        evidence.append(
            PersonProfileEvidence(
                key=key,
                claim=f"{attribute.predicate}: {attribute.value}",
                epistemic=attribute.epistemic.model_dump_json(),
                validity=attribute.validity.model_dump_json()
                if attribute.validity
                else "unspecified",
            )
        )
    for index, reference in enumerate(intent.claim_refs):
        claim = await claim_in(self.cognition, view, ctx, reference)
        body = claim.body
        current = await view.connection.scalar(
            select(revisions.c.head).where(
                revisions.c.id == reference.id,
                revisions.c.revision == reference.revision,
            )
        )
        if (
            not current
            or claim.deleted
            or not isinstance(body, FactClaimBody)
            or body.status != "active"
            or body.stability != "stable"
            or not isinstance(body.scope, PersonFactScope)
            or body.scope.person.id != record.reference.id
            or claim.times.validity is not None
            and not claim.times.validity.contains(ctx.clock)
        ):
            raise DomainError(
                "profile_evidence_changed", "A frozen profile claim has changed or expired"
            )
        key = f"F{index + 1}"
        catalog[key] = (EvidenceLink(reference=reference, role="fact_claim"),)
        evidence.append(
            PersonProfileEvidence(
                key=key,
                claim=f"{body.fact_key} ({body.polarity}): {body.value_text}",
                epistemic=canonical_json(
                    {
                        "classification": "reported",
                        "authority": body.authority,
                        "confidence": body.confidence,
                    }
                ),
                validity=claim.times.validity.model_dump_json()
                if claim.times.validity
                else "unspecified",
            )
        )
    roots = await evidence_sources(
        self, view, ctx, tuple(link for links in catalog.values() for link in links)
    )
    return PersonProfileModelInput(name=record.body.label, evidence=tuple(evidence)), catalog, roots


async def run_refresh(self: PersonProfileService, payload: JsonValue) -> JsonValue:
    intent = PersonProfileRefreshIntent.model_validate(payload)
    ctx, command = intent.context, intent.command
    adoption_id = command.operation_id + ":adopt"
    receipt = await self.authority.receipt(
        ctx,
        adoption_id,
        operation_kind="person_profile.form",
        scope=ctx.require_subject(),
        payload=intent,
    )
    if receipt is not None:
        return receipt.result
    async with self.authority.read(ctx) as view:
        variables, catalog, source_ids = await profile_input_in(self, view, intent)
    call_id: str | None = None
    support: tuple[EvidenceLink, ...] = ()
    summary, aliases = "", ()
    if catalog:
        generated = await self.models.generate(
            ctx,
            "person_profile",
            variables.model_dump(),
            PersonProfileDraft,
            source_ids=source_ids,
            invocation_id="person-profile:" + command.operation_id,
        )
        if set(generated.output.evidence_keys).difference(catalog):
            raise DomainError(
                "profile_evidence_unknown",
                "The profile cites an unknown evidence key",
                call_id=generated.call_id,
            )
        support = tuple(
            dict.fromkeys(link for key in generated.output.evidence_keys for link in catalog[key])
        )
        summary, aliases, call_id = (
            generated.output.summary,
            generated.output.aliases,
            generated.call_id,
        )
    async with self.authority.write(
        ctx,
        adoption_id,
        operation_kind="person_profile.form",
        scope=ctx.require_subject(),
        payload=intent,
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        await profile_input_in(self, tx, intent)
        previous = await self.person_in(tx, ctx, command.reference)
        assert isinstance(previous.body, EntityBody)
        body = previous.body.model_copy(update={"description": summary, "aliases": aliases})
        value = await self.revise_in(
            tx,
            ctx,
            previous,
            body,
            (*previous.evidence, *support),
            action="person_profile.refresh" if catalog else "person_profile.withdraw",
            reason="Explicit single-person profile refresh"
            if catalog
            else "No retained profile evidence remains",
            producer_call_id=call_id,
        )
        result = tx.result(
            TypeAdapter(JsonValue).validate_python(
                {
                    "reference": value.reference.model_dump(mode="json"),
                    "call_id": call_id,
                    "cleared_no_evidence": not bool(catalog),
                }
            )
        )
    if self.cognition.invalidate:
        await self.cognition.invalidate()
    return result
