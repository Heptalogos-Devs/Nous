"""Person controls are source-backed Entity revisions, never a second profile store."""

from __future__ import annotations

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
    RevisionCause,
    TimePoint,
)
from nous_sdk.contracts.cognition import PersonAliasOverride, PersonSummaryOverride
from nous_sdk.contracts.lifecycle import MemoryChange
from nous_sdk.contracts.person_profiles import PersonProfileChange, PersonProfileChangeResult

from nous.identity.service import require_subject_in
from nous.platform.persistence import Authority, ReadView, WriteTx

from . import person_refresh
from .revision_state import RevisionState

if TYPE_CHECKING:
    from .models import ModelServices
    from .service import CognitionService


class PersonProfileService:
    submit_refresh = person_refresh.submit_refresh
    submit_refresh_in = person_refresh.submit_refresh_in
    run_refresh = person_refresh.run_refresh

    def __init__(self, authority: Authority, cognition: CognitionService, models: ModelServices):
        self.authority, self.cognition, self.models = authority, cognition, models

    async def output_context(
        self,
        ctx: CallContext,
        command: PersonProfileChange,
        result: PersonProfileChangeResult,
    ) -> CallContext:
        async with self.authority.read(ctx) as view:
            return (
                await self.cognition.qualify_known_outputs_in(
                    view,
                    ctx,
                    (result.record.reference,),
                    operation_id=command.operation_id,
                    operation_kind="person_profile.change",
                    scope=ctx.require_subject(),
                    input_material_ids=(result.report_material_id,),
                )
                if ctx.source_groups()
                else ctx
            )

    async def person_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
    ) -> CognitiveRecord:
        record = await self.cognition.get_in(view, ctx, reference)
        if not isinstance(record.body, EntityBody) or record.body.entity_type != "person":
            raise DomainError("person_required", "Profile operations require a Person Entity")
        if record.deleted or record.purged:
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
        producer_call_id: str | None = None,
    ) -> CognitiveRecord:
        value = await self.cognition.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=body,
                evidence=evidence,
                times=previous.times,
                producer_call_id=producer_call_id,
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
            async with self.authority.write(
                ctx,
                command.operation_id,
                operation_kind="person_profile.change",
                scope=ctx.require_subject(),
                payload=command,
            ) as tx:
                if tx.replayed:
                    result = PersonProfileChangeResult.model_validate(tx.replay_result)
                    effective = (
                        await self.cognition.qualify_known_outputs_in(
                            tx,
                            ctx,
                            (result.record.reference,),
                            operation_id=command.operation_id,
                            operation_kind="person_profile.change",
                            scope=ctx.require_subject(),
                            input_material_ids=(result.report_material_id,),
                        )
                        if ctx.source_groups()
                        else ctx
                    )
                    await self.cognition.replay_record_in(tx, effective, result.record)
                    return result
                await require_subject_in(tx, ctx, "memory")
                previous = await self.person_in(tx, ctx, command.reference)
                assert isinstance(previous.body, EntityBody)
                now = TimePoint(
                    coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
                )
                admission = await self.cognition.materials.admit_text_in(
                    tx,
                    ctx,
                    document.model_copy(update={"observed_at": now}),
                    lease,
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
                                aliases=tuple(dict.fromkeys(command.aliases)),
                                evidence=(report,),
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
                            "summary": PersonSummaryOverride(
                                text=command.text,
                                evidence=(report,),
                            )
                        }
                    )
                elif command.action == "delete_override":
                    controls = controls.model_copy(update={"summary": None})
                body = previous.body.model_copy(update={"profile_controls": controls})
                if command.action == "correct_evidence":
                    body = corrected_body(body, command)
                # Controls carry their own support. Old administrative values are not reattached.
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
                    corrected_memory = await self.cognition.change_memory_in(
                        tx,
                        ctx,
                        MemoryChange(
                            reference=command.memory_reference,
                            action="trash",
                            reason=command.reason,
                        ),
                    )
                result = tx.result(
                    PersonProfileChangeResult(
                        record=value,
                        corrected_memory=corrected_memory,
                        report_material_id=admission.material["id"],
                        observation_id=admission.event["id"],
                        commit_seq=tx.commit_seq,
                    )
                )
            if self.cognition.invalidate:
                await self.cognition.invalidate()
            return result
        finally:
            if lease and store:
                await store.release(lease)


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
        return body.model_copy(update={"attributes": tuple(attributes), "description": ""})
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
    return body.model_copy(update={"attributes": tuple(attributes), "description": ""})
