"""Only this owner changes canonical fact identity, classification and conflict state."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    DomainError,
    EvidenceLink,
    ImportDocument,
    RevisionCause,
    TimePoint,
    TimeRange,
)
from nous_sdk.contracts.cognitive_identity import CognitiveIdentity
from nous_sdk.contracts.fact_body import (
    ChatFactScope,
    FactClaimBody,
    FactClaimSpec,
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

from nous.identity.service import require_subject_in
from nous.platform.persistence import Authority, WriteTx

from . import fact_changes
from .fact_identity import normalized, scope_identity
from .fact_reads import claim_in, head_claim_in, heads_in, query_in, view_in
from .graph_state import heads_in as cognitive_heads_in
from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from .person_profiles import PersonProfileService
    from .service import CognitionService


class FactService:
    apply_in = fact_changes.apply_in

    def __init__(
        self, authority: Authority, cognition: CognitionService, profiles: PersonProfileService
    ):
        self.authority, self.cognition, self.profiles = authority, cognition, profiles

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
            await self.profiles.person_in(tx, ctx, reference)
            canonical = PersonFactScope(person=reference)
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
                revisions.c.subject_id == ctx.subject_id,
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
    ) -> CognitiveRecord:
        now = TimePoint(coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000))
        times = (
            previous.times
            if previous
            else CognitiveTimes(formation=now, observation=now, recorded=now)
        )
        if apply_requested_validity and "validity" in command.model_fields_set:
            times = times.model_copy(update={"validity": command.validity})
        if apply_requested_validity and command.validity_patch is not None:
            if "validity" in command.model_fields_set:
                raise DomainError(
                    "fact_validity_ambiguous", "Choose a full validity range or a partial update"
                )
            patch, old = command.validity_patch, times.validity
            if old is not None and old.domain != patch.domain:
                raise DomainError(
                    "fact_validity_domain",
                    "Partial validity updates must keep the original clock domain",
                )
            times = times.model_copy(
                update={
                    "validity": TimeRange(
                        domain=patch.domain,
                        start=patch.start
                        if "start" in patch.model_fields_set
                        else old.start
                        if old
                        else None,
                        end=patch.end
                        if "end" in patch.model_fields_set
                        else old.end
                        if old
                        else None,
                    )
                }
            )
        validity = times.validity
        if action in {"retract", "supersede"}:
            validity = validity or TimeRange(domain=now.domain)
            if (
                validity.domain == now.domain
                and validity.end is None
                and (validity.start is None or validity.start <= now.coordinate)
            ):
                times = times.model_copy(
                    update={"validity": validity.model_copy(update={"end": now.coordinate})}
                )
        elif action == "restore" and validity is not None:
            times = times.model_copy(update={"validity": validity.model_copy(update={"end": None})})
        if action in {"assert", "reinforce", "restore"}:
            times = times.model_copy(update={"observation": now})
        body = body.model_copy(
            update={
                "first_observed": body.first_observed if previous else now,
                "last_confirmed": now
                if action in {"assert", "reinforce", "restore"}
                else body.last_confirmed,
            }
        )
        value = await self.cognition.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id if previous else None,
                expected_revision=previous.reference.revision if previous else 0,
                body=body,
                evidence=(
                    *(previous.evidence if previous else ()),
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
            async with self.authority.write(
                ctx,
                command.operation_id,
                operation_kind="fact.change",
                scope=ctx.require_subject(),
                payload=command.model_dump(mode="json", exclude_unset=True),
            ) as tx:
                if tx.replayed:
                    result = FactChangeResult.model_validate(tx.replay_result)
                    effective = (
                        await self.cognition.qualify_known_outputs_in(
                            tx,
                            ctx,
                            (
                                result.record.reference,
                                *(record.reference for record in result.changed),
                            ),
                            operation_id=command.operation_id,
                            operation_kind="fact.change",
                            scope=ctx.require_subject(),
                            input_material_ids=(result.report_material_id,),
                        )
                        if ctx.source_groups()
                        else ctx
                    )
                    for record in (result.record, *result.changed):
                        await self.cognition.replay_record_in(tx, effective, record)
                    return result
                await require_subject_in(tx, ctx, "memory")
                # Every pre-existing target is checked in the incoming context.
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
                    else (
                        FactClaimSpec.model_validate(
                            previous.body.model_dump(include=set(FactClaimSpec.model_fields))
                        )
                        if previous and isinstance(previous.body, FactClaimBody)
                        else None
                    )
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
                value = tx.result(
                    FactChangeResult(
                        record=result,
                        changed=changed,
                        conflicting=conflicts,
                        report_material_id=admission.material["id"],
                        observation_id=admission.event["id"],
                        commit_seq=tx.commit_seq,
                        profile_job_id=profile_job_id,
                    )
                )
            if self.cognition.invalidate:
                await self.cognition.invalidate()
            return value
        finally:
            if lease and store:
                await store.release(lease)

    async def query(self, ctx: CallContext, command: FactQuery) -> tuple[CognitiveRecord, ...]:
        async with self.authority.read(ctx) as view:
            return await query_in(self.cognition, view, ctx, command)

    async def get(self, ctx: CallContext, reference: CognitiveRef | str) -> FactView:
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            exact = (
                reference
                if isinstance(reference, CognitiveRef)
                else (await head_claim_in(self.cognition, view, ctx, reference)).reference
            )
            return await view_in(self.cognition, view, ctx, exact)
