"""MaiBot profile actions consume source-backed Person operations."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.amemorix.contracts import ProfileAdminInput, ProfileInput
from nous_wave.contracts import CallContext, CognitiveRecord, CognitiveRef, DomainError, EntityBody
from nous_wave.contracts.context import CognitiveTarget
from nous_wave.contracts.person_profiles import PersonProfileChange, RefreshPersonProfile
from pydantic import JsonValue

from .helpers import json_object
from .profile_read import aliases, people, read_profile, select_person, summary

if TYPE_CHECKING:
    from .service import AMemorixService


async def refresh_person(
    self: AMemorixService,
    ctx: CallContext,
    record: CognitiveRecord,
    command: ProfileInput,
) -> CognitiveRecord:
    if not command.operation_id:
        raise DomainError("operation_id_required", "Profile refresh requires a stable operation_id")
    submitted = await self.app.person_profiles.submit_refresh(
        ctx,
        RefreshPersonProfile(
            operation_id=command.operation_id + ":refresh",
            reference=record.reference,
        ),
    )
    result = await self.app.execution.wait(ctx, submitted.job_id)
    if not isinstance(result, dict):
        raise DomainError(
            "profile_refresh_result_invalid", "Profile formation returned no reference"
        )
    return await self.app.cognition.get(ctx, CognitiveRef.model_validate(result["reference"]))


async def profile_admin(
    self: AMemorixService,
    ctx: CallContext,
    arguments: dict[str, JsonValue],
) -> dict[str, JsonValue]:
    command = ProfileAdminInput.model_validate(arguments)
    if command.action == "list":
        if command.force_refresh:
            raise DomainError("person_required", "Forced refresh requires one explicit Person")
        limit = command.limit if "limit" in arguments or "top_k" in arguments else 50
        items = [summary(record) for record in (await people(self, ctx))[:limit]]
        return json_object({"success": True, "items": items, "count": len(items)})
    record = await selected_intent(self, ctx, command)
    if command.action == "correct_evidence" and command.evidence_type == "fact_claim":
        assert command.operation_id is not None
        from nous_wave.contracts.fact_body import FactClaimBody, PersonFactScope
        from nous_wave.contracts.facts import FactChange

        token = command.evidence_key or command.hash or ""
        reference = await cognitive_reference(self, ctx, token)
        fact = await self.app.facts.get(ctx, reference)
        body = fact.record.body
        if (
            not isinstance(body, FactClaimBody)
            or not isinstance(body.scope, PersonFactScope)
            or body.scope.person.id != record.reference.id
        ):
            raise DomainError("fact_scope_mismatch", "The fact must belong to the selected Person")
        changed = await self.app.facts.change(
            ctx,
            FactChange(
                operation_id=command.operation_id + ":fact",
                action="retract",
                reference=reference,
                reason=command.reason,
                reported_by=command.requested_by,
                refresh_profile=command.refresh,
            ),
        )
        if changed.profile_job_id:
            result = await self.app.execution.wait(ctx, changed.profile_job_id)
            if not isinstance(result, dict):
                raise DomainError(
                    "profile_refresh_result_invalid", "Profile refresh returned no reference"
                )
            record = await self.app.cognition.get(
                ctx, CognitiveRef.model_validate(result["reference"])
            )
        return json_object(
            {
                "success": True,
                "operation_id": command.operation_id,
                "fact": changed.record,
                "refresh_queued": changed.profile_job_id is not None,
                "profile_job_id": changed.profile_job_id,
                "refreshed_profile": summary(record),
                "refreshed_evidence": await read_profile(self, ctx, record, command),
            }
        )
    if command.action in (
        "set_aliases",
        "delete_aliases",
        "set_override",
        "delete_override",
        "correct_evidence",
    ):
        if not command.operation_id:
            raise DomainError(
                "operation_id_required", "Profile changes require a stable operation_id"
            )
        pointer, reference, memory = command.attribute_pointer, record.reference, None
        if command.reference is not None:
            if command.reference.kind == "entity":
                reference = command.reference
            elif command.action == "correct_evidence" and command.reference.kind == "memory":
                memory = command.reference
            else:
                raise DomainError(
                    "profile_evidence_required",
                    "Select a Person or an actual Memory evidence reference",
                )
        token = command.evidence_key or command.hash
        if command.action == "correct_evidence" and token:
            if "#/body/attributes/" in token:
                address, path = token.split("#", 1)
                reference = await cognitive_reference(self, ctx, address)
                pointer = path
            elif command.evidence_type in {"memory", "paragraph"}:
                memory = command.reference or await cognitive_reference(self, ctx, token)
            else:
                raise DomainError(
                    "profile_evidence_required",
                    "Select the exact evidence_key returned by evidence",
                )
        if (
            command.action == "correct_evidence"
            and pointer
            and not token
            and command.reference is None
        ):
            raise DomainError(
                "profile_evidence_required",
                "An attribute_pointer requires its exact Entity revision",
            )
        if reference.id != record.reference.id:
            raise DomainError(
                "profile_evidence_mismatch", "The selected attribute belongs to another Person"
            )
        change = PersonProfileChange(
            operation_id=command.operation_id,
            reference=reference,
            action=command.action,
            aliases=command.aliases,
            text=command.text,
            attribute_pointer=pointer,
            evidence=command.evidence,
            memory_reference=memory,
            reason=command.reason,
            reported_by=command.requested_by or command.source,
        )
        changed = await self.app.person_profiles.change(ctx, change)
        ctx = await self.app.person_profiles.output_context(ctx, change, changed)
        record = changed.record
        response: dict[str, object] = {
            "success": True,
            "operation_id": command.operation_id,
            "reference": record.reference,
            "operation": changed,
            "refresh_queued": False,
        }
        if command.action.endswith("aliases"):
            response.update(await aliases(self, ctx, record))
            assert isinstance(record.body, EntityBody)
            response["override"] = record.body.profile_controls.aliases
        else:
            response.update(summary(record))
            assert isinstance(record.body, EntityBody)
            response["override"] = record.body.profile_controls.summary
        if command.action == "correct_evidence" and command.refresh:
            try:
                record = await refresh_person(self, ctx, record, command)
            except DomainError as error:
                if error.code != "profile_evidence_required":
                    raise
                response["refresh_error"] = {"code": error.code, "message": str(error)}
            else:
                response.update(summary(record))
        elif command.force_refresh:
            record = await refresh_person(self, ctx, record, command)
            response.update(summary(record))
        if command.action == "correct_evidence":
            response["refreshed_evidence"] = await read_profile(self, ctx, record, command)
            response["refreshed_profile"] = summary(record)
        return json_object(response)
    if command.force_refresh:
        record = await refresh_person(self, ctx, record, command)
    if command.action == "get_aliases":
        return json_object(await aliases(self, ctx, record))
    result = await read_profile(self, ctx, record, command)
    if command.action == "query":
        result["profile"] = summary(record)
    return json_object(result)


async def cognitive_reference(
    self: AMemorixService, ctx: CallContext, address: str
) -> CognitiveRef:
    target = (await self.app.references.resolve(ctx, address)).reference
    if not isinstance(target, CognitiveTarget):
        raise DomainError(
            "profile_evidence_required", "Select an exact cognitive evidence reference"
        )
    return target.reference


async def selected_intent(
    self: AMemorixService,
    ctx: CallContext,
    command: ProfileAdminInput,
) -> CognitiveRecord:
    if command.action in {"query", "evidence", "get_aliases"} and not command.force_refresh:
        return await select_person(self, ctx, command)
    if not command.operation_id:
        raise DomainError(
            "operation_id_required", "Profile mutations require a stable operation_id"
        )
    async with self.app.authority.write(
        ctx,
        command.operation_id + ":selection",
        operation_kind="amemorix.profile.select",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            reference = CognitiveRef.model_validate(tx.replay_result)
        else:
            record = await select_person(self, ctx, command)
            reference = record.reference
            tx.result(reference)
        return await self.app.cognition.get_in(tx, ctx, reference)
