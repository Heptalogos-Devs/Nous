"""Person controls are source-backed Entity revisions, never a second profile store."""

from __future__ import annotations

import unicodedata
from datetime import datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EntityAttribute,
    EntityBody,
    EvidenceLink,
    ImportDocument,
    RetentionChange,
    RevisionCause,
    TimePoint,
)
from nous_sdk.contracts.cognition import (
    GeneratedPersonProfile,
    PersonAliasOverride,
    PersonSummaryOverride,
)
from nous_sdk.contracts.fact_body import FactClaimBody, PersonFactScope
from nous_sdk.contracts.facts import FactQuery
from nous_sdk.contracts.person_profiles import (
    PersonProfileChange,
    PersonProfileChangeResult,
    PersonProfileDraft,
    PersonProfileEvidence,
    PersonProfileEvidenceItem,
    PersonProfileModelInput,
    PersonProfileRefreshIntent,
    PersonProfileRefreshSubmission,
    PersonProfilesQuery,
    PersonProfilesResult,
    PersonProfileView,
    RefreshPersonProfile,
)
from nous_sdk.contracts.records import RecordsQuery
from nous_sdk.contracts.common import ModelProvenance
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.material.service import require_observation_in
from nous.platform.execution import JobSpec
from nous.platform.persistence import Authority, ReadView, WriteTx

from .fact_reads import query_in as fact_query_in
from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from .models import ModelServices
    from .records import CognitiveRecords
    from .memory import MemoryService


def grams(value: str) -> set[str]:
    normalized = unicodedata.normalize("NFKC", value).casefold()
    token = "".join(char for char in normalized if char.isalnum())
    return {token[index : index + 2] for index in range(len(token) - 1)} - {
        "用户",
        "这个",
        "那个",
        "自己",
        "喜欢",
    }


_UNAVAILABLE = {"source_unavailable", "cognition_not_found", "region_not_found"}


class PersonProfileService:
    def __init__(
        self, authority: Authority, cognition: CognitiveRecords, models: ModelServices | None, *, memories: MemoryService
    ):
        self.memories = memories
        self.authority, self.cognition, self.models = (authority, cognition, models)

    async def output_context(
        self, ctx: CallContext, command: PersonProfileChange, result: PersonProfileChangeResult
    ) -> CallContext:
        async with self.authority.read(ctx) as view:
            return (
                await self.cognition.qualify_known_outputs_in(
                    view,
                    ctx,
                    (result.record.reference,),
                    observation_ids=(result.observation_id,),
                    input_material_ids=(result.report_material_id,),
                )
                if ctx.source_groups()
                else ctx
            )

    async def person_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
    ) -> CognitiveRecord:
        await require_namespace_in(view, ctx, "memory")
        record = await self.cognition.read_in(view, ctx, reference)
        if not isinstance(record.body, EntityBody) or record.body.entity_type != "person":
            raise DomainError("person_required", "Profile operations require a Person Entity")
        if (
            record.deleted
            or record.purged
            or await self.cognition.automatic_in(view, ctx, reference) is None
        ):
            raise DomainError("person_unavailable", "The Person is retired or purged")
        return record

    async def revise_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        previous: CognitiveRecord,
        body: EntityBody,
        evidence: tuple[EvidenceLink, ...],
        *,
        action: str,
        reason: str,
        producer: ModelProvenance | None = None,
    ) -> CognitiveRecord:

        await require_namespace_in(tx, ctx, "person_profiles")
        value = await self.cognition.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=body,
                evidence=evidence,
                times=previous.times,
                producer=producer,
            ),
            state=RevisionState(
                cause=RevisionCause(
                    action=action, actor=ctx.caller_id, reason=reason, parent=previous.reference
                ),
                lifecycle=previous.lifecycle,
                deleted=previous.deleted,
            ),
        )
        return value

    async def stored_person_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
    ) -> CognitiveRecord:
        await require_namespace_in(view, ctx, "memory")
        record = await self.cognition.get_in(view, ctx, reference, purpose="current")
        if not isinstance(record.body, EntityBody) or record.body.entity_type != "person":
            raise DomainError("person_required", "Profile operations require a Person Entity")
        return record

    async def change(
        self, ctx: CallContext, command: PersonProfileChange
    ) -> PersonProfileChangeResult:
        document = ImportDocument(
            text=command.model_dump_json(),
            format="json",
            filename="person-profile-change.json",
            media_type="application/json",
            source_type="administrative_change",
            source="person_profile_management",
            origin={
                "caller_id": ctx.caller_id,
                **({"reported_by": command.reported_by} if command.reported_by else {}),
            },
        )
        store = self.cognition.materials.content_store
        lease = (
            await store.publish_bytes(document.inline_text().encode(), document.media_type)
            if store
            else None
        )
        try:
            async with self.authority.write(ctx) as tx:
                await require_namespace_in(tx, ctx, "memory")
                previous = await self.person_in(tx, ctx, command.reference)
                assert isinstance(previous.body, EntityBody)
                now = TimePoint(
                    coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
                )
                admission = await self.cognition.materials.admit_text_in(
                    tx, ctx, document.model_copy(update={"observed_at": now}), lease
                )
                report = EvidenceLink(observation_id=admission.event["id"], role=command.action)
                controls = previous.body.profile_controls
                if command.action == "set_aliases":
                    if command.aliases is None:
                        raise DomainError(
                            "aliases_required",
                            "Supply aliases; an empty list is an explicit override",
                        )
                    controls = controls.model_copy(
                        update={
                            "aliases": PersonAliasOverride(
                                aliases=tuple(dict.fromkeys(command.aliases)), evidence=(report,)
                            )
                        }
                    )
                elif command.action == "delete_aliases":
                    controls = controls.model_copy(update={"aliases": None})
                elif command.action == "set_override":
                    if command.text is None:
                        raise DomainError("profile_text_required", "Supply the manual profile text")
                    controls = controls.model_copy(
                        update={
                            "summary": PersonSummaryOverride(text=command.text, evidence=(report,))
                        }
                    )
                elif command.action == "delete_override":
                    controls = controls.model_copy(update={"summary": None})
                body = previous.body.model_copy(update={"profile_controls": controls})
                if command.action == "correct_evidence":
                    body = corrected_body(body, command)
                value = await self.revise_in(
                    tx,
                    admission.context,
                    previous,
                    body,
                    (
                        *(
                            link
                            for link in previous.evidence
                            if command.memory_reference is None
                            or link.reference != command.memory_reference
                        ),
                        report,
                    ),
                    action="person_profile." + command.action,
                    reason=command.reason,
                )
                corrected_memory = None
                if command.action == "correct_evidence" and command.memory_reference is not None:
                    corrected_memory = await self.memories.change_retention_in(
                        tx,
                        ctx,
                        RetentionChange(
                            reference=command.memory_reference,
                            action="trash",
                            reason=command.reason,
                        ),
                    )
                result = PersonProfileChangeResult(record=value, corrected_memory=corrected_memory, report_material_id=admission.material['id'], observation_id=admission.event['id'], commit_seq=tx.commit_seq)
            return result
        finally:
            if lease and store:
                await store.release(lease)

    async def people(self, ctx: CallContext) -> tuple[CognitiveRecord, ...]:
        records: list[CognitiveRecord] = []
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "person_profiles")
            command = RecordsQuery(kinds=("entity",), limit=1000)
            while True:
                page = await self.cognition.records_in(view, ctx, command)
                records.extend(
                    item.record
                    for item in page.items
                    if isinstance(item.record.body, EntityBody)
                    and item.record.body.entity_type == "person"
                )
                if page.next_cursor is None:
                    return tuple(sorted(records, key=lambda record: record.reference.id))
                command = command.model_copy(update={"cursor": page.next_cursor})

    async def select(self, ctx: CallContext, command: PersonProfilesQuery) -> CognitiveRecord:
        if command.reference is not None:
            async with self.authority.read(ctx) as view:
                await require_namespace_in(view, ctx, "person_profiles")
                return await self.person_in(view, ctx, command.reference)
        if not command.identifier and (not command.name):
            raise DomainError(
                "person_required", "Select a Person reference, identifier or exact name"
            )
        found: list[CognitiveRecord] = []
        for record in await self.people(ctx):
            assert isinstance(record.body, EntityBody)
            body = record.body
            identifiers = (
                set(body.identifiers.values())
                if command.platform is None
                else {
                    value
                    for key, value in body.identifiers.items()
                    if key.startswith(command.platform + ":")
                }
            )
            matched = (
                command.identifier
                in {
                    record.reference.id,
                    record.reference.exact(),
                    *identifiers,
                    *(
                        identity.account_id
                        for identity in body.platform_identities
                        if command.platform is None or identity.platform == command.platform
                    ),
                }
                if command.identifier
                else command.name.casefold().strip()
                in {name.casefold().strip() for name in (body.label, *body.effective_aliases)}
            )
            if matched:
                found.append(record)
        if len(found) != 1:
            raise DomainError(
                "person_not_found" if not found else "person_ambiguous",
                "Person selector has no unique authorized match",
            )
        return found[0]

    async def read(
        self, ctx: CallContext, record: CognitiveRecord, command: PersonProfilesQuery
    ) -> PersonProfileView:
        accepted: list[PersonProfileEvidenceItem] = []
        uncertain: list[PersonProfileEvidenceItem] = []
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "person_profiles")
            record = await self.person_in(view, ctx, record.reference)
            assert isinstance(record.body, EntityBody)
            if record.body.description:
                accepted.append(
                    PersonProfileEvidenceItem(
                        key=f"{record.reference.exact()}#/body/description",
                        kind="entity_description",
                        content=record.body.description,
                        reference=record.reference,
                        evidence=record.evidence,
                    )
                )
            for index, attribute in enumerate(record.body.attributes):
                try:
                    for link in attribute.evidence:
                        if link.reference is not None:
                            await self.cognition.get_in(view, ctx, link.reference)
                        elif link.observation_id is not None:
                            await require_observation_in(
                                view, ctx, link.observation_id, region_id=link.region_id
                            )
                except DomainError as error:
                    if error.code not in _UNAVAILABLE:
                        raise
                    continue
                item = PersonProfileEvidenceItem(
                    key=f"{record.reference.exact()}#/body/attributes/{index}",
                    kind="entity_attribute",
                    content=f"{attribute.predicate}: {attribute.value}",
                    reference=record.reference,
                    attribute_pointer=f"/body/attributes/{index}",
                    evidence=attribute.evidence,
                    epistemic=attribute.epistemic,
                    validity=attribute.validity,
                )
                confirmed = (
                    attribute.epistemic.classification in {"observed", "reported"}
                    and (not any(link.polarity == "counterexample" for link in attribute.evidence))
                    and (attribute.validity is None or attribute.validity.contains(ctx.clock))
                )
                (accepted if confirmed else uncertain).append(item)
            claims = await fact_query_in(
                self.cognition,
                view,
                ctx,
                FactQuery(
                    scope=PersonFactScope(person=record.reference),
                    statuses=("active", "conflicted"),
                    limit=max(1, command.evidence_limit),
                ),
            )
            for claim in claims:
                body = claim.body
                assert isinstance(body, FactClaimBody)
                item = PersonProfileEvidenceItem(
                    key=claim.reference.exact(),
                    kind="fact_claim",
                    reference=claim.reference,
                    content=f"{body.fact_key} ({body.polarity}): {body.value_text}",
                    evidence=(*claim.evidence, *(entry.link for entry in body.evidence)),
                    status=body.status,
                    stability=body.stability,
                    authority=body.authority,
                    confidence=body.confidence,
                    validity=claim.times.validity,
                )
                confirmed = (
                    body.status == "active"
                    and body.stability == "stable"
                    and (
                        not any(entry.link.polarity == "counterexample" for entry in body.evidence)
                    )
                    and (claim.times.validity is None or claim.times.validity.contains(ctx.clock))
                )
                (accepted if confirmed else uncertain).append(item)
            if command.context_text:
                context = grams(command.context_text)
                uncertain.sort(key=lambda item: (-len(context & grams(item.content)), item.key))
            return PersonProfileView(
                record=record,
                evidence=tuple(accepted[: command.evidence_limit]),
                uncertain=tuple(uncertain[: command.evidence_limit]),
                evidence_count=len(accepted),
                uncertain_count=len(uncertain),
                watermark=view.commit_seq,
            )

    async def query(self, ctx: CallContext, command: PersonProfilesQuery) -> PersonProfilesResult:
        selected = (
            (await self.select(ctx, command),)
            if command.reference is not None or command.identifier or command.name
            else (await self.people(ctx))[: command.limit]
        )
        return PersonProfilesResult(
            items=tuple([await self.read(ctx, record, command) for record in selected])
        )

    async def evidence_sources(
        self, view: ReadView | WriteTx, ctx: CallContext, links: tuple[EvidenceLink, ...]
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
                await require_observation_in(
                    view, ctx, link.observation_id, region_id=link.region_id
                )
                roots.add(link.observation_id)
        return tuple(sorted(roots))

    async def submit_refresh(
        self, ctx: CallContext, command: RefreshPersonProfile
    ) -> PersonProfileRefreshSubmission:
        async with self.authority.write(ctx) as tx:
            return await self.submit_refresh_in(tx, ctx, command)

    async def submit_refresh_in(
        self, tx: WriteTx, ctx: CallContext, command: RefreshPersonProfile
    ) -> PersonProfileRefreshSubmission:
        from .fact_reads import current_person_claims_in

        await require_namespace_in(tx, ctx, "memory")

        await require_namespace_in(tx, ctx, "person_profiles")
        record = await self.stored_person_in(tx, ctx, command.reference)
        assert isinstance(record.body, EntityBody)
        indices: list[int] = []
        roots = await self.cognition.source_roots_in(tx, ctx)
        description_selected = bool(
            record.body.description
            and await self.cognition.live_links_in(tx, ctx, record.evidence, roots)
        )
        capacity = command.evidence_limit - int(description_selected)
        for index, attribute in enumerate(record.body.attributes):
            if len(indices) >= capacity:
                break
            links = await self.cognition.live_links_in(tx, ctx, attribute.evidence, roots)
            if not links:
                continue
            try:
                await self.evidence_sources(tx, ctx, links)
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
        claims = await current_person_claims_in(self.cognition, tx, ctx, record.reference.id)
        intent = PersonProfileRefreshIntent(
            context=ctx,
            command=command,
            attribute_indices=tuple(indices),
            claim_refs=tuple(
                claim.reference for claim in claims[: max(0, capacity - len(indices))]
            ),
            description_selected=description_selected,
        )
        job = await tx.enqueue(
            JobSpec(
                "nous.person_profile.refresh", "formation", args=(intent.model_dump(mode="json"),)
            )
        )
        return PersonProfileRefreshSubmission(job_id=job.workflow_id, reference=record.reference)

    async def profile_input_in(
        self, view: ReadView | WriteTx, intent: PersonProfileRefreshIntent
    ) -> tuple[PersonProfileModelInput, dict[str, tuple[EvidenceLink, ...]], tuple[str, ...]]:
        from nous_sdk.contracts.fact_body import FactClaimBody, PersonFactScope

        from nous.platform.persistence.authority import canonical_json

        from .fact_reads import claim_in

        ctx, command = (intent.context, intent.command)
        record = await self.stored_person_in(view, ctx, command.reference)
        assert isinstance(record.body, EntityBody)
        head = await view.connection.scalar(
            select(revisions.c.head).where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.id == record.reference.id, revisions.c.revision == record.reference.revision)
        )
        if not head:
            raise DomainError(
                "revision_conflict", "The Person changed after its refresh was frozen"
            )
        catalog: dict[str, tuple[EvidenceLink, ...]] = {}
        evidence: list[PersonProfileEvidence] = []
        active_roots = await self.cognition.source_roots_in(view, ctx)
        description_links = await self.cognition.live_links_in(
            view, ctx, record.evidence, active_roots
        )
        if intent.description_selected and record.body.description and description_links:
            roots = await self.evidence_sources(view, ctx, description_links)
            if roots:
                catalog["D0"] = description_links
                evidence.append(
                    PersonProfileEvidence(
                        key="D0",
                        claim=record.body.description,
                        epistemic="source-backed entity description",
                        validity="unspecified",
                    )
                )
        for index, position in enumerate(intent.attribute_indices):
            attribute = record.body.attributes[position]
            links = await self.cognition.live_links_in(view, ctx, attribute.evidence, active_roots)
            if not links:
                raise DomainError(
                    "profile_evidence_changed",
                    "A frozen Entity attribute no longer has live support",
                )
            key = f"E{index + 1}"
            catalog[key] = links
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
                select(revisions.c.head).where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.id == reference.id, revisions.c.revision == reference.revision)
            )
            if (
                not current
                or claim.deleted
                or (not isinstance(body, FactClaimBody))
                or (body.status != "active")
                or (body.stability != "stable")
                or (not isinstance(body.scope, PersonFactScope))
                or (body.scope.person.id != record.reference.id)
                or (
                    claim.times.validity is not None
                    and (not claim.times.validity.contains(ctx.clock))
                )
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
        roots = await self.evidence_sources(
            view, ctx, tuple(link for links in catalog.values() for link in links)
        )
        return (
            PersonProfileModelInput(name=record.body.label, evidence=tuple(evidence)),
            catalog,
            roots,
        )

    async def run_refresh(self, payload: JsonValue) -> JsonValue:
        intent = PersonProfileRefreshIntent.model_validate(payload)
        ctx, command = (intent.context, intent.command)
        adoption_id = command.operation_id + ":adopt"
        async with self.authority.read(ctx) as view:
            variables, catalog, source_ids = await self.profile_input_in(view, intent)
        producer: ModelProvenance | None = None
        support: tuple[EvidenceLink, ...] = ()
        summary, aliases = ("", ())
        if set(catalog) - {"D0"}:
            if self.models is None:
                raise DomainError(
                    "model_unavailable",
                    "Person profile generation requires a configured model service",
                )
            generated = await self.models.generate(
                ctx,
                "person_profile",
                variables.model_dump(),
                PersonProfileDraft,
                source_ids=source_ids,
            )
            if set(generated.output.evidence_keys).difference(catalog):
                raise DomainError(
                    "profile_evidence_unknown",
                    "The profile cites an unknown evidence key",
                    output_digest=generated.producer.output_digest,
                )
            support = tuple(
                dict.fromkeys(
                    link for key in generated.output.evidence_keys for link in catalog[key]
                )
            )
            summary, aliases, producer = (
                generated.output.summary,
                generated.output.aliases,
                generated.producer,
            )
        async with self.authority.write(ctx) as tx:
            await self.profile_input_in(tx, intent)
            previous = await self.stored_person_in(tx, ctx, command.reference)
            assert isinstance(previous.body, EntityBody)
            profile = (
                GeneratedPersonProfile(
                    summary=summary, aliases=aliases, evidence=support, producer=producer
                )
                if producer is not None
                else None
            )
            body = previous.body.model_copy(update={"generated_profile": profile})
            if body == previous.body:
                return {'reference': previous.reference.model_dump(mode='json'), 'producer': producer.model_dump(mode='json') if producer else None, 'cleared_no_evidence': not bool(catalog), 'unchanged': True}
            value = await self.revise_in(
                tx,
                ctx,
                previous,
                body,
                previous.evidence,
                action="person_profile.refresh" if producer else "person_profile.withdraw",
                reason="Explicit single-person profile refresh"
                if producer
                else "No retained attributes or stable claims remain for automatic synthesis",
                producer=producer,
            )
            result = TypeAdapter(JsonValue).validate_python({'reference': value.reference.model_dump(mode='json'), 'producer': producer.model_dump(mode='json') if producer else None, 'cleared_no_evidence': not bool(catalog)})
        return result


def corrected_body(body: EntityBody, command: PersonProfileChange) -> EntityBody:
    if command.memory_reference is not None:
        if command.memory_reference.kind != "memory":
            raise DomainError(
                "memory_required", "Only actual Memory evidence can use Memory lifecycle"
            )
        attributes: list[EntityAttribute] = []
        found = False
        for attribute in body.attributes:
            links = tuple(
                link for link in attribute.evidence if link.reference != command.memory_reference
            )
            found |= links != attribute.evidence
            if any(link.polarity == "support" for link in links):
                attributes.append(attribute.model_copy(update={"evidence": links}))
        if not found:
            raise DomainError(
                "profile_evidence_not_found", "The exact Memory does not support this Person"
            )
        return body.model_copy(update={"attributes": tuple(attributes), "generated_profile": None})
    prefix = "/body/attributes/"
    pointer = command.attribute_pointer or ""
    if not pointer.startswith(prefix) or not pointer[len(prefix) :].isdigit():
        raise DomainError("profile_evidence_required", "Select an exact Entity attribute_pointer")
    index = int(pointer[len(prefix) :])
    if index >= len(body.attributes):
        raise DomainError("profile_evidence_not_found", "The exact attribute does not exist")
    attributes = list(body.attributes)
    attribute = attributes[index]
    if command.evidence is None:
        del attributes[index]
    else:
        if command.evidence not in attribute.evidence:
            raise DomainError(
                "profile_evidence_not_found", "The selected support is absent from this attribute"
            )
        links = tuple(link for link in attribute.evidence if link != command.evidence)
        if not any(link.polarity == "support" for link in links):
            del attributes[index]
        else:
            attributes[index] = attribute.model_copy(update={"evidence": links})
    return body.model_copy(update={"attributes": tuple(attributes), "generated_profile": None})
