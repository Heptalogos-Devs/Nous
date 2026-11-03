"""Only this owner changes canonical fact identity, classification and conflict state."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    EntityBody,
    EvidenceLink,
    ImportDocument,
    RevisionCause,
    TimePoint,
    TimeRange,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.cognitive_identity import CognitiveIdentity
from nous_sdk.contracts.fact_body import (
    ChatFactScope,
    FactClaimBody,
    FactClaimSpec,
    FactEvidence,
    FactStatus,
    NamespaceFactScope,
    PersonFactScope,
)
from nous_sdk.contracts.facts import (
    FactChange,
    FactChangeResult,
    FactClaimInput,
    FactQuery,
    FactView,
    PersonFactScopeInput,
)
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.platform.persistence import Authority, WriteTx

from .fact_identity import (
    ClaimIdentity,
    claim_identity,
    conflict_group,
    conflicts_with,
    identity,
    normalized,
    requested_validity,
    scope_identity,
)
from .fact_reads import claim_in, head_claim_in, heads_in, query_in, view_in
from .graph_state import heads_in as cognitive_heads_in
from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from .person_profiles import PersonProfileService
    from .records import CognitiveRecords


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


class FactService:
    async def revise_evidence_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        previous: CognitiveRecord,
        *,
        add: tuple[EvidenceLink, ...],
        remove: tuple[EvidenceLink, ...],
        reason: str,
    ) -> CognitiveRecord:
        await self.head_in(tx, ctx, previous.reference)
        body = previous.body
        assert body is not None
        links = tuple(
            dict.fromkeys((*(link for link in previous.evidence if link not in remove), *add))
        )
        if isinstance(body, FactClaimBody):
            body = body.model_copy(
                update={
                    "evidence": tuple(item for item in body.evidence if item.link not in remove)
                    + tuple(
                        FactEvidence(link=link)
                        for link in add
                        if not any(item.link == link for item in body.evidence)
                    )
                }
            )
        if body == previous.body and links == previous.evidence:
            return previous
        return await self.cognition.commit_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=body,
                evidence=(
                    *links,
                    EvidenceLink(
                        reference=previous.reference, polarity="boundary", role="evidence history"
                    ),
                ),
                times=previous.times,
                producer=previous.producer,
            ),
            state=RevisionState(
                deleted=previous.deleted,
                lifecycle=previous.lifecycle,
                cause=RevisionCause(
                    action="evidence.change",
                    actor=ctx.caller_id,
                    reason=reason,
                    parent=previous.reference,
                ),
            ),
        )

    async def assert_source_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        claim: FactClaimInput,
        origin: CognitiveRecord,
        validity: TimeRange | None = None,
        *,
        validity_explicit: bool = False,
    ) -> tuple[CognitiveRecord, ...]:
        from nous_sdk.contracts.fact_body import FactEvidence

        await self.cognition.get_in(tx, ctx, origin.reference)
        claim = claim.model_copy(
            update={
                "authority": "summary_derived"
                if claim.authority == "summary_derived"
                else "imported"
            }
        )
        evidence = EvidenceLink(reference=origin.reference)
        spec = await self.specification_in(tx, ctx, claim)
        candidates = await heads_in(tx, ctx, spec.scope)
        command = FactChange(
            action="assert",
            claim=claim,
            evidence=(FactEvidence(link=evidence),),
            refresh_profile=False,
            reason="Explicit source assertion",
        )
        if validity_explicit:
            command = command.model_copy(update={"validity": validity})
        value, changed, _ = await self.apply_in(
            tx, ctx, command, spec, None, candidates, (), evidence, origin=origin
        )
        return (value, *changed)

    def __init__(
        self, authority: Authority, cognition: CognitiveRecords, profiles: PersonProfileService
    ):
        self.authority, self.cognition, self.profiles = (authority, cognition, profiles)

    async def specification_in(
        self, tx: WriteTx, ctx: CallContext, value: FactClaimInput
    ) -> FactClaimSpec:
        scope = value.scope
        if isinstance(scope, PersonFactScopeInput):
            reference = scope.person
            if isinstance(reference, CognitiveIdentity):
                if reference.kind != "entity":
                    raise DomainError("person_required", "Person scope requires an Entity identity")
                records = await cognitive_heads_in(
                    tx, ctx, ("entity",), reference_ids=(reference.id,)
                )
                if not records:
                    raise DomainError("person_not_found", "Person identity is unavailable")
                reference = records[0].reference
            person = await self.cognition.read_in(tx, ctx, reference, purpose="current")
            if not isinstance(person.body, EntityBody) or person.body.entity_type != "person":
                raise DomainError(
                    "person_required", "Person scope requires a current Person Entity"
                )
            canonical = PersonFactScope(person=reference)
        elif isinstance(scope, NamespaceFactScope):
            canonical = scope
        else:
            canonical = ChatFactScope(conversation_ref=scope.conversation_ref)
        return FactClaimSpec.model_validate(
            {**value.model_dump(exclude={"scope"}), "scope": canonical.model_dump()}
        )

    async def head_in(
        self, tx: WriteTx, ctx: CallContext, reference: CognitiveRef
    ) -> CognitiveRecord:
        value = await claim_in(self.cognition, tx, ctx, reference)
        current = await tx.connection.scalar(
            select(revisions.c.revision).where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.id == reference.id,
                revisions.c.head.is_(True),
            )
        )
        if current != reference.revision:
            raise DomainError(
                "revision_conflict", "The FactClaim changed since the expected revision"
            )
        return value

    async def revise_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        body: FactClaimBody,
        previous: CognitiveRecord | None,
        report: EvidenceLink,
        action: str,
        command: FactChange,
        *,
        apply_requested_validity: bool = True,
        extra_evidence: tuple[EvidenceLink, ...] = (),
        origin: CognitiveRecord | None = None,
        inherit_support: bool = True,
    ) -> CognitiveRecord:
        now = TimePoint(coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000))
        times = (
            previous.times
            if previous
            else origin.times
            if origin
            else CognitiveTimes(formation=now, observation=now, recorded=now)
        )
        if apply_requested_validity:
            times = times.model_copy(
                update={"validity": requested_validity(command, times.validity)}
            )
        if action in {"assert", "reinforce", "restore"} and origin is None:
            times = times.model_copy(update={"observation": now})
        body = body.model_copy(
            update={
                "first_observed": body.first_observed
                if previous
                else origin.times.observation
                if origin
                else now,
                "last_confirmed": now
                if action in {"assert", "reinforce", "restore"}
                else body.last_confirmed,
            }
        )
        value = await self.cognition.commit_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id if previous else None,
                expected_revision=previous.reference.revision if previous else 0,
                body=body,
                evidence=(
                    *(
                        link
                        if inherit_support or link.polarity != "support"
                        else link.model_copy(update={"polarity": "boundary"})
                        for link in (previous.evidence if previous else ())
                    ),
                    *(
                        (
                            EvidenceLink(
                                reference=previous.reference,
                                role="fact_history",
                                polarity="boundary",
                            ),
                        )
                        if previous
                        else ()
                    ),
                    report,
                    *extra_evidence,
                ),
                times=times,
            ),
            state=RevisionState(
                cause=RevisionCause(
                    action="fact." + action,
                    actor=ctx.caller_id,
                    reason=command.reason,
                    parent=previous.reference if previous else None,
                )
            ),
        )
        return value

    async def restore_correction_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        current: CognitiveRecord,
        before: CognitiveRecord | None,
        report: EvidenceLink,
        reason: str,
    ) -> CognitiveRecord:
        """Undo this operation's claim transition while retaining later conflicting evidence."""
        await self.head_in(tx, ctx, current.reference)
        assert isinstance(current.body, FactClaimBody)
        if before is None:
            return await self.revise_in(
                tx,
                ctx,
                current.body.model_copy(update={"status": "retracted"}),
                current,
                report,
                "retract",
                FactChange(action="retract", reason=reason),
            )
        assert isinstance(before.body, FactClaimBody)
        body = before.body
        if body.status == "active":
            candidates = await heads_in(tx, ctx, body.scope)
            if any(
                candidate.reference.id != current.reference.id
                and isinstance(candidate.body, FactClaimBody)
                and (candidate.body.status in {"active", "conflicted"})
                and conflicts_with(
                    candidate.body, candidate.times.validity, body, before.times.validity
                )
                for candidate in candidates
            ):
                body = body.model_copy(update={"status": "conflicted"})
        return await self.cognition.commit_in(
            tx,
            ctx,
            AdoptCognition(
                id=current.reference.id,
                expected_revision=current.reference.revision,
                body=body,
                evidence=(
                    *before.evidence,
                    report,
                    EvidenceLink(
                        reference=current.reference, polarity="boundary", role="fact.rollback"
                    ),
                ),
                times=before.times,
            ),
            state=RevisionState(
                cause=RevisionCause(
                    action="fact.rollback",
                    actor=ctx.caller_id,
                    reason=reason,
                    parent=current.reference,
                )
            ),
        )

    async def change(self, ctx: CallContext, command: FactChange) -> FactChangeResult:
        document = ImportDocument(
            text=command.model_dump_json(exclude_unset=True),
            format="json",
            filename="fact-change.json",
            media_type="application/json",
            source_type="administrative_change",
            source="fact_management",
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
                await require_namespace_in(tx, ctx, capability="memory")
                if command.reference is not None and command.claim_id is not None:
                    raise DomainError(
                        "fact_reference_ambiguous", "Select one exact reference or stable claim_id"
                    )
                previous = (
                    await self.head_in(tx, ctx, command.reference)
                    if command.reference
                    else await head_claim_in(self.cognition, tx, ctx, command.claim_id)
                    if command.claim_id
                    else None
                )
                spec = (
                    await self.specification_in(tx, ctx, command.claim)
                    if command.claim
                    else FactClaimSpec.model_validate(
                        previous.body.model_dump(include=set(FactClaimSpec.model_fields))
                    )
                    if previous and isinstance(previous.body, FactClaimBody)
                    else None
                )
                if command.patch is not None:
                    if spec is None or command.claim is not None:
                        raise DomainError(
                            "fact_patch_invalid", "A patch requires one existing claim"
                        )
                    fields = {
                        name: getattr(command.patch, name)
                        for name in command.patch.model_fields_set
                        if getattr(command.patch, name) is not None or name == "confidence"
                    }
                    spec = FactClaimSpec.model_validate({**spec.model_dump(), **fields})
                if spec is None:
                    raise DomainError(
                        "fact_claim_required",
                        "The operation requires a claim or exact existing reference",
                    )
                if (
                    command.expected_scope_kind is not None
                    and spec.scope.kind != command.expected_scope_kind
                ):
                    raise DomainError(
                        "fact_scope_immutable", "A claim update cannot change its person/chat scope"
                    )
                if command.expected_scope is not None:
                    expected = await self.specification_in(
                        tx,
                        ctx,
                        FactClaimInput.model_validate(
                            {
                                **spec.model_dump(exclude={"scope"}),
                                "scope": command.expected_scope.model_dump(),
                            }
                        ),
                    )
                    if scope_identity(expected.scope) != scope_identity(spec.scope):
                        raise DomainError(
                            "fact_scope_immutable",
                            "A claim update cannot change its person/chat scope",
                        )
                if not normalized(spec.fact_key) or not normalized(spec.value_text):
                    raise DomainError(
                        "fact_value_required", "Fact keys and values must contain text"
                    )
                if isinstance(spec.scope, PersonFactScope):
                    await self.profiles.person_in(tx, ctx, spec.scope.person)
                candidates: list[CognitiveRecord] = []
                for candidate in await heads_in(tx, ctx, spec.scope):
                    if candidate.purged or candidate.deleted:
                        continue
                    candidates.append(await self.cognition.get_in(tx, ctx, candidate.reference))
                supersedes = tuple([await self.head_in(tx, ctx, ref) for ref in command.supersedes])
                admission = await self.cognition.materials.admit_text_in(tx, ctx, document, lease)
                report = EvidenceLink(
                    observation_id=admission.event["id"], role="fact." + command.action
                )
                result, changed, conflicts = await self.apply_in(
                    tx,
                    admission.context,
                    command,
                    spec,
                    previous,
                    tuple(candidates),
                    supersedes,
                    report,
                )
                profile_job_id = None
                if command.refresh_profile and isinstance(spec.scope, PersonFactScope):
                    from nous_sdk.contracts.person_profiles import RefreshPersonProfile

                    people = await cognitive_heads_in(
                        tx, admission.context, ("entity",), reference_ids=(spec.scope.person.id,)
                    )
                    person = await self.profiles.person_in(
                        tx, admission.context, people[0].reference
                    )
                    submission = await self.profiles.submit_refresh_in(
                        tx,
                        admission.context,
                        RefreshPersonProfile(
                            operation_id=command.operation_id + ":profile",
                            reference=person.reference,
                        ),
                    )
                    profile_job_id = submission.job_id
                value = FactChangeResult(record=result, changed=changed, conflicting=conflicts, report_material_id=admission.material['id'], observation_id=admission.event['id'], commit_seq=tx.commit_seq, profile_job_id=profile_job_id)
            return value
        finally:
            if lease and store:
                await store.release(lease)

    async def query(self, ctx: CallContext, command: FactQuery) -> tuple[CognitiveRecord, ...]:
        async with self.authority.read(ctx) as view:
            return await query_in(self.cognition, view, ctx, command)

    async def get(self, ctx: CallContext, reference: CognitiveRef | str) -> FactView:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            exact = (
                reference
                if isinstance(reference, CognitiveRef)
                else (await head_claim_in(self.cognition, view, ctx, reference)).reference
            )
            return await view_in(self.cognition, view, ctx, exact)

    async def apply_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        command: FactChange,
        spec: FactClaimSpec,
        requested: CognitiveRecord | None,
        candidates: tuple[CognitiveRecord, ...],
        explicit_supersedes: tuple[CognitiveRecord, ...],
        report: EvidenceLink,
        *,
        replace_support: bool = False,
        origin: CognitiveRecord | None = None,
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
        validity = requested_validity(
            command,
            requested.times.validity if requested else origin.times.validity if origin else None,
        )
        target = next(
            (
                record
                for record in candidates
                if isinstance(record.body, FactClaimBody)
                and claim_identity(record.body, record.times.validity)
                == claim_identity(spec, validity)
            ),
            None,
        )
        if (
            requested is not None
            and isinstance(requested.body, FactClaimBody)
            and (identity(requested.body) == identity(spec))
        ):
            if target is not None and target.reference.id != requested.reference.id:
                raise DomainError(
                    "fact_validity_collision",
                    "Another claim already owns this content and applicability",
                )
            target = requested
        if target is not None:
            assert isinstance(target.body, FactClaimBody)
            if target.body.cardinality != spec.cardinality:
                raise DomainError(
                    "fact_cardinality_immutable", "An existing claim cannot change cardinality"
                )
        group = conflict_group(spec)
        supersedes = list(explicit_supersedes)
        changed: list[CognitiveRecord] = []
        if requested is not None and (
            target is None or target.reference.id != requested.reference.id
        ):
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
            and record.body.status in {"active", "conflicted"}
            and conflicts_with(record.body, record.times.validity, spec, validity)
            and (record.reference.id not in retired_ids)
            and (target is None or record.reference.id != target.reference.id)
        )
        if operation != "assert" and requested is None:
            raise DomainError(
                "fact_reference_required", "A transition requires the exact current claim"
            )
        prior = target.body if target is not None else None
        assert prior is None or isinstance(prior, FactClaimBody)
        evidence = merged_evidence(
            tuple(
                item
                for item in prior.evidence
                if not replace_support or item.link.polarity != "support"
            )
            if prior
            else (),
            command.evidence,
        )
        status: FactStatus = "conflicted" if conflicts else "active"
        action = "reinforce" if target else "assert"
        if operation in {"classify", "add_evidence"}:
            assert prior is not None
            status, action = (prior.status, operation)
            if (
                prior.status in {"active", "conflicted"}
                and target is not None
                and (validity != target.times.validity)
            ):
                status = "conflicted" if conflicts else "active"
            if operation == "add_evidence" and (not command.evidence):
                raise DomainError(
                    "fact_evidence_required", "Adding evidence requires its actual source"
                )
        elif operation == "retract":
            status, action = ("retracted", "retract")
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
                    (
                        *(prior.supersedes if prior else ()),
                        *(record.reference for record in supersedes),
                    )
                )
            ),
            first_observed=prior.first_observed if prior else None,
            last_confirmed=prior.last_confirmed if prior else None,
            identity_lineage=prior.identity_lineage if prior else (),
        )
        context = (
            (
                EvidenceLink(
                    reference=requested.reference, polarity="boundary", role="fact.revision"
                ),
            )
            if requested is not None
            and (target is None or requested.reference.id != target.reference.id)
            else ()
        )
        value = await self.revise_in(
            tx,
            ctx,
            body,
            target,
            report,
            action,
            command,
            extra_evidence=context,
            inherit_support=not replace_support,
            origin=origin,
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
                            reference=value.reference,
                            role="fact.superseded_by",
                            polarity="boundary",
                        ),
                    ),
                )
            )
        return (value, tuple(changed), conflicts)

    async def reassign_person_claims_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        sources: tuple[CognitiveRef, ...],
        target: CognitiveRef,
        reason: str,
        *,
        claims: tuple[CognitiveRef, ...] | None = None,
    ) -> tuple[tuple[CognitiveRecord, ...], dict[str, CognitiveRef]]:
        await self.profiles.person_in(tx, ctx, target)
        selected = {ref.id: ref for ref in claims} if claims is not None else None
        found: dict[str, CognitiveRecord] = {}
        groups: dict[ClaimIdentity, list[CognitiveRecord]] = {}
        for source in sources:
            for before in await heads_in(tx, ctx, PersonFactScope(person=source)):
                if (
                    before.deleted
                    or before.purged
                    or (not isinstance(before.body, FactClaimBody))
                    or (before.body.status not in {"active", "conflicted"})
                ):
                    continue
                if selected is not None and before.reference.id not in selected:
                    continue
                if selected is not None and selected[before.reference.id] != before.reference:
                    raise DomainError(
                        "revision_conflict", "A selected claim changed before identity migration"
                    )
                await self.cognition.get_in(tx, ctx, before.reference)
                body = before.body.model_copy(update={"scope": PersonFactScope(person=target)})
                groups.setdefault(claim_identity(body, before.times.validity), []).append(before)
                found[before.reference.id] = before
        if selected is not None and set(selected) != set(found):
            raise DomainError(
                "entity_claim_assignment",
                "Each assigned claim must be a current claim of the split entity",
            )
        changes: list[CognitiveRecord] = []
        mapping: dict[str, CognitiveRef] = {}
        report = EvidenceLink(reference=target, polarity="boundary", role=reason)
        incoming = [
            (
                record.body.model_copy(update={"scope": PersonFactScope(person=target)}),
                record.times.validity,
            )
            for values in groups.values()
            for record in values
            if isinstance(record.body, FactClaimBody)
        ]
        for key, originals in groups.items():
            before = originals[0]
            assert isinstance(before.body, FactClaimBody)
            body = before.body.model_copy(
                update={
                    "scope": PersonFactScope(person=target),
                    "supersedes": (),
                    "identity_lineage": tuple(
                        dict.fromkeys(
                            ref
                            for record in originals
                            if isinstance(record.body, FactClaimBody)
                            for ref in (*record.body.identity_lineage, record.reference)
                        )
                    ),
                }
            )
            candidates = [
                record
                for record in await heads_in(tx, ctx, body.scope)
                if not record.deleted
                and (not record.purged)
                and isinstance(record.body, FactClaimBody)
            ]
            previous = next(
                (
                    record
                    for record in candidates
                    if isinstance(record.body, FactClaimBody)
                    and claim_identity(record.body, record.times.validity) == key
                ),
                None,
            )
            evidence = ()
            for record in originals:
                assert isinstance(record.body, FactClaimBody)
                if record.body.cardinality != body.cardinality:
                    raise DomainError(
                        "fact_cardinality_immutable",
                        "Reconcile claim cardinalities before merging identities",
                    )
                evidence = merged_evidence(evidence, record.body.evidence)
            if previous is not None:
                await self.head_in(tx, ctx, previous.reference)
                assert isinstance(previous.body, FactClaimBody)
                if previous.body.cardinality != body.cardinality:
                    raise DomainError(
                        "fact_cardinality_immutable",
                        "Reconcile claim cardinalities before merging identities",
                    )
                evidence = merged_evidence(previous.body.evidence, evidence)
                body = body.model_copy(
                    update={
                        "identity_lineage": tuple(
                            dict.fromkeys((*previous.body.identity_lineage, *body.identity_lineage))
                        )
                    }
                )
            conflicting = [
                record
                for record in candidates
                if isinstance(record.body, FactClaimBody)
                and record.body.status in {"active", "conflicted"}
                and conflicts_with(record.body, record.times.validity, body, before.times.validity)
            ]
            conflict = bool(conflicting) or any(
                (
                    conflicts_with(other, validity, body, before.times.validity)
                    for other, validity in incoming
                )
            )
            body = body.model_copy(
                update={"evidence": evidence, "status": "conflicted" if conflict else "active"}
            )
            command = FactChange(action="classify", reason=reason, refresh_profile=False)
            migrated = await self.revise_in(
                tx,
                ctx,
                body,
                previous,
                report,
                "migrate_scope",
                command,
                apply_requested_validity=False,
                origin=before,
                extra_evidence=tuple(link for record in originals for link in record.evidence)
                + tuple(
                    EvidenceLink(
                        reference=record.reference, polarity="boundary", role="identity migration"
                    )
                    for record in originals
                ),
            )
            changes.append(migrated)
            for record in conflicting:
                assert isinstance(record.body, FactClaimBody)
                if record.body.status != "conflicted":
                    changes.append(
                        await self.revise_in(
                            tx,
                            ctx,
                            record.body.model_copy(update={"status": "conflicted"}),
                            record,
                            report,
                            "classify",
                            command,
                            apply_requested_validity=False,
                        )
                    )
            for original in originals:
                assert isinstance(original.body, FactClaimBody)
                mapping[original.reference.id] = migrated.reference
                changes.append(
                    await self.revise_in(
                        tx,
                        ctx,
                        original.body.model_copy(update={"status": "superseded"}),
                        original,
                        report,
                        "migrate_scope",
                        command,
                        apply_requested_validity=False,
                        extra_evidence=(
                            EvidenceLink(
                                reference=migrated.reference,
                                polarity="boundary",
                                role="identity successor",
                            ),
                        ),
                    )
                )
        return (tuple({record.reference.id: record for record in changes}.values()), mapping)
