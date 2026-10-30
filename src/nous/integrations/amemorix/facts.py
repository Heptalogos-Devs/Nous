"""A fact-admin is a translation of canonical claim operations and immutable history."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.amemorix.facts import FactAdminInput
from nous_sdk.contracts import CallContext, CognitiveRecord, DomainError, TimePoint, TimeRange
from nous_sdk.contracts.cognitive_identity import CognitiveIdentity
from nous_sdk.contracts.fact_body import ChatFactScope, FactClaimBody, PersonFactScope
from nous_sdk.contracts.facts import (
    ChatFactScopeInput,
    FactChange,
    FactClaimInput,
    FactClaimPatch,
    FactQuery,
    FactScopeInput,
    FactView,
    PersonFactScopeInput,
)
from pydantic import JsonValue

from nous.cognition.fact_identity import normalized

from .helpers import json_object, point

if TYPE_CHECKING:
    from .service import AMemorixService


def seconds(value: TimePoint | None) -> float | None:
    return value.coordinate / 1000 if value is not None and value.domain == "unix_ms" else None


def claim_record(record: CognitiveRecord) -> dict[str, JsonValue]:
    body = record.body
    assert isinstance(body, FactClaimBody)
    validity = record.times.validity
    return json_object(
        {
            "claim_id": record.reference.id,
            "reference": record.reference,
            "revision": record.reference.revision,
            "scope_type": body.scope.kind,
            "scope_id": body.scope.identity,
            "fact_key": body.fact_key,
            "value_text": body.value_text,
            "value_normalized": normalized(body.value_text),
            "polarity": body.polarity,
            "cardinality": body.cardinality,
            "stability": body.stability,
            "profile_section": body.profile_section,
            "authority": body.authority,
            "status": body.status,
            "confidence": body.confidence,
            "valid_from": validity.start / 1000
            if validity and validity.domain == "unix_ms" and validity.start is not None
            else None,
            "valid_to": validity.end / 1000
            if validity and validity.domain == "unix_ms" and validity.end is not None
            else None,
            "validity": validity,
            "first_observed_at": seconds(body.first_observed),
            "last_confirmed_at": seconds(body.last_confirmed),
            "created_at": seconds(record.times.formation),
            "updated_at": seconds(record.times.recorded),
            "evidence": body.evidence,
            "source_roots": record.source_roots,
            "supersedes": body.supersedes,
        }
    )


def claim_view(view: FactView) -> dict[str, JsonValue]:
    value = claim_record(view.record)
    value["successors"] = [reference.model_dump(mode="json") for reference in view.successors]
    value["conflicting_claim_ids"] = [reference.id for reference in view.conflicting]
    value["transitions"] = [
        json_object(
            {
                "transition_id": record.reference.exact(),
                "old_claim_id": record.cause.parent.id
                if record.cause and record.cause.parent
                else None,
                "new_claim_id": next(
                    (
                        link.reference.id
                        for link in record.evidence
                        if link.role == "fact.superseded_by" and link.reference
                    ),
                    None,
                )
                if isinstance(record.body, FactClaimBody) and record.body.status == "superseded"
                else None
                if isinstance(record.body, FactClaimBody) and record.body.status == "retracted"
                else record.reference.id,
                "transition_type": record.cause.action.removeprefix("fact.")
                if record.cause
                else "import",
                "reason": record.cause.reason if record.cause else "",
                "created_at": seconds(record.times.recorded),
            }
        )
        for record in view.history
    ]
    return value


async def scope_input(
    self: AMemorixService, ctx: CallContext, command: FactAdminInput
) -> FactScopeInput:
    if not command.scope_id:
        raise DomainError("fact_scope_required", "Facts require an exact person/chat scope_id")
    if command.scope_type == "chat":
        return ChatFactScopeInput(conversation_ref=command.scope_id)
    if command.scope_type != "person":
        raise DomainError("fact_scope_invalid", "Fact scope_type must be person or chat")
    from nous_sdk.amemorix.contracts import ProfileInput

    from .profile_read import select_person

    person = await select_person(self, ctx, ProfileInput(person_id=command.scope_id))
    return PersonFactScopeInput(person=CognitiveIdentity(kind="entity", id=person.reference.id))


async def fact_admin(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = FactAdminInput.model_validate(arguments)
    owner = self.app.facts
    if command.action == "get":
        view = await owner.get(ctx, command.claim_id)
        return {"success": True, "claim": claim_view(view)}
    if command.action == "list":
        scope = await scope_input(self, ctx, command)
        if isinstance(scope, ChatFactScopeInput):
            selected = ChatFactScope(conversation_ref=scope.conversation_ref)
        else:
            from nous_sdk.amemorix.contracts import ProfileInput

            from .profile_read import select_person

            person = await select_person(self, ctx, ProfileInput(person_id=command.scope_id))
            selected = PersonFactScope(person=person.reference)
        rows = await owner.query(
            ctx,
            FactQuery(
                scope=selected, statuses=command.statuses or ("active",), limit=command.limit
            ),
        )
        items = [claim_record(record) for record in rows]
        return json_object({"success": True, "items": items, "count": len(items)})
    if command.action not in {"create", "update", "retract", "restore"}:
        raise DomainError(
            "capability_unavailable", "Unsupported fact action", action=command.action
        )
    if not command.operation_id:
        raise DomainError("operation_id_required", "Fact writes require a stable operation_id")
    values: dict[str, object] = {
        "operation_id": command.operation_id,
        "action": "assert" if command.action == "create" else command.action,
        "reason": command.reason,
        "reported_by": command.requested_by or command.updated_by,
        "refresh_profile": command.refresh_profile,
    }
    if command.action == "create":
        values["claim"] = FactClaimInput.model_validate(
            {
                "scope": (await scope_input(self, ctx, command)).model_dump(),
                "fact_key": command.fact_key,
                "value_text": command.value_text,
                "polarity": command.polarity or "positive",
                "cardinality": command.cardinality or "set",
                "stability": command.stability or "stable",
                "profile_section": command.profile_section or "stable_facts",
                "authority": command.authority or "manual",
                "confidence": command.confidence if command.confidence is not None else 1.0,
            }
        )
    else:
        if not command.claim_id:
            raise DomainError("fact_claim_required", "This action requires claim_id")
        values["claim_id"] = command.claim_id
        if command.action == "update":
            values["patch"] = FactClaimPatch.model_validate(
                {
                    key: value
                    for key in FactClaimPatch.model_fields
                    if key in command.model_fields_set
                    and (value := getattr(command, key)) is not None
                }
            )
            if "scope_id" in command.model_fields_set:
                values["expected_scope"] = await scope_input(self, ctx, command)
            if "scope_type" in command.model_fields_set:
                values["expected_scope_kind"] = command.scope_type
    if (
        "valid_from" in command.model_fields_set
        or "valid_to" in command.model_fields_set
        or command.action == "create"
    ):
        start, end = point(command.valid_from), point(command.valid_to)
        if command.action == "create":
            values["validity"] = TimeRange(
                start=start.coordinate if start else None, end=end.coordinate if end else None
            )
        else:
            bounds: dict[str, object] = {}
            if "valid_from" in command.model_fields_set:
                bounds["start"] = start.coordinate if start else None
            if "valid_to" in command.model_fields_set:
                bounds["end"] = end.coordinate if end else None
            values["validity_patch"] = TimeRange.model_validate(bounds)
    result = await owner.change(ctx, FactChange.model_validate(values))
    return json_object(
        {
            "success": True,
            "claim": claim_record(result.record),
            "reference": result.record.reference,
            "previous_claim_id": command.claim_id or None,
            "replaced": bool(command.claim_id and command.claim_id != result.record.reference.id),
            "refresh_queued": result.profile_job_id is not None,
            "profile_job_id": result.profile_job_id,
            "conflicting_claim_ids": [reference.id for reference in result.conflicting],
            "changed": [
                claim_record(record)
                for record in result.changed
                if isinstance(record.body, FactClaimBody)
            ],
            "operation": result,
        }
    )
