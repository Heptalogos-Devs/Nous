"""Freeze one Person's exact evidence and adopt its actual paid summary once."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, DomainError, EntityBody, EvidenceLink
from nous_wave.contracts.person_profiles import (
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
        if not indices:
            raise DomainError(
                "profile_evidence_required", "No lawful Person attributes are available to refresh"
            )
        intent = PersonProfileRefreshIntent(
            context=ctx, command=command, attribute_indices=tuple(indices)
        )
        job = await tx.enqueue(
            JobSpec(
                "nous.person_profile.refresh",
                "formation",
                args=(intent.model_dump(mode="json"),),
            )
        )
        return tx.result(
            PersonProfileRefreshSubmission(job_id=job.workflow_id, reference=record.reference)
        )


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
        record = await self.person_in(view, ctx, command.reference)
        head = await view.connection.scalar(
            select(revisions.c.head).where(
                revisions.c.id == record.reference.id,
                revisions.c.revision == record.reference.revision,
            )
        )
        if not head:
            raise DomainError(
                "revision_conflict", "The Person changed after its refresh was frozen"
            )
        assert isinstance(record.body, EntityBody)
        attributes = [record.body.attributes[index] for index in intent.attribute_indices]
        links = tuple(link for attribute in attributes for link in attribute.evidence)
        source_ids = await evidence_sources(self, view, ctx, links)
        variables = PersonProfileModelInput(
            name=record.body.label,
            evidence=tuple(
                PersonProfileEvidence(
                    key=f"E{index + 1}",
                    claim=f"{attribute.predicate}: {attribute.value}",
                    epistemic=attribute.epistemic.model_dump_json(),
                    validity=attribute.validity.model_dump_json()
                    if attribute.validity
                    else "unspecified",
                )
                for index, attribute in enumerate(attributes)
            ),
        )
    generated = await self.models.generate(
        ctx,
        "person_profile",
        variables.model_dump(),
        PersonProfileDraft,
        source_ids=source_ids,
        invocation_id="person-profile:" + command.operation_id,
    )
    catalog = {f"E{index + 1}": attribute for index, attribute in enumerate(attributes)}
    if set(generated.output.evidence_keys).difference(catalog):
        raise DomainError(
            "profile_evidence_unknown",
            "The profile cites an unknown evidence key",
            call_id=generated.call_id,
        )
    support = tuple(
        dict.fromkeys(
            link for key in generated.output.evidence_keys for link in catalog[key].evidence
        )
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
        previous = await self.person_in(tx, ctx, command.reference)
        assert isinstance(previous.body, EntityBody)
        await evidence_sources(self, tx, ctx, links)
        body = previous.body.model_copy(
            update={
                "description": generated.output.summary,
                "aliases": generated.output.aliases,
            }
        )
        value = await self.revise_in(
            tx,
            ctx,
            previous,
            body,
            (*previous.evidence, *support),
            action="person_profile.refresh",
            reason="Explicit single-person profile refresh",
            producer_call_id=generated.call_id,
        )
        result = tx.result(
            TypeAdapter(JsonValue).validate_python(
                {"reference": value.reference.model_dump(mode="json"), "call_id": generated.call_id}
            )
        )
    if self.cognition.invalidate:
        await self.cognition.invalidate()
    return result
