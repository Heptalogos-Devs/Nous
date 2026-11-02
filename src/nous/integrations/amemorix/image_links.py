"""Map image links to the target object's native evidence revisions."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.amemorix.contracts import ImageInput
from nous_sdk.contracts import CallContext, DomainError, EvidenceLink
from nous_sdk.contracts.context import CognitiveTarget
from nous_sdk.contracts.source_objects import EvidenceReference, EvidenceRelationRecord
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def change_link(
    service: AMemorixService, ctx: CallContext, command: ImageInput
) -> dict[str, JsonValue]:
    async with service.app.authority.write(
        ctx,
        command.operation_id,
        operation_kind="cognition.source_link",
        scope=ctx.namespace_id,
        payload=command,
    ) as tx:
        if tx.replayed:
            result = json_object(tx.replay_result)
            link = result.get("link")
            identifier = link.get("link_id") if isinstance(link, dict) else None
            if isinstance(identifier, str):
                await service.app.references.read_source_object_in(
                    tx, ctx, EvidenceReference(id=identifier.removeprefix("evidence:"))
                )
            return result
        if command.action == "link":
            if not command.occurrence_id or not command.target_id:
                raise DomainError("image_link_required", "Select an actual occurrence and target")
            accepted = await service.app.material.accepted_source_in(
                tx, ctx, command.occurrence_id, derivations=service.app.derivations
            )
            if not accepted.material["media_type"].startswith("image/"):
                raise DomainError(
                    "image_required", "Image attachment requires an actual image occurrence"
                )
            kind = {"paragraph": "memory", "relation": "association", "fact": "fact_claim"}.get(
                command.target_type, command.target_type
            )
            locator = (
                command.target_id
                if ":" in command.target_id
                else f"{kind}:{command.target_id}"
                if kind
                else command.target_id
            )
            target = (await service.app.references.resolve_in(tx, ctx, locator)).reference
            if not isinstance(target, CognitiveTarget) or kind and target.reference.kind != kind:
                raise DomainError(
                    "cognitive_target_required",
                    "Image links require the selected native cognitive kind",
                )
            link = EvidenceLink(
                observation_id=command.occurrence_id,
                polarity="support" if command.link_kind in {"evidence", "support"} else "boundary",
                role="media.link:" + command.link_kind,
                details=command.evidence,
            )
            value = await service.app.cognition.revise_evidence_in(
                tx, ctx, target.reference, add=(link,), reason=command.reason
            )
            identifier = EvidenceReference(
                id=f"{value.reference.exact()}#{value.evidence.index(link)}"
            ).exact()
            return tx.result(
                json_object(
                    {
                        "success": True,
                        "link": {
                            "link_id": identifier,
                            "occurrence_id": command.occurrence_id,
                            "target_type": value.reference.kind,
                            "target_id": value.reference.id,
                            "target": value.reference,
                            "link_kind": command.link_kind,
                            "evidence": command.evidence,
                        },
                    }
                )
            )
        if not command.link_id:
            raise DomainError("image_link_required", "Select an exact evidence link")
        reference = EvidenceReference(id=command.link_id.removeprefix("evidence:"))
        record = await service.app.references.read_source_object_in(tx, ctx, reference)
        if not isinstance(record, EvidenceRelationRecord) or not record.evidence.role.startswith(
            "media.link:"
        ):
            raise DomainError(
                "image_link_required", "The evidence relation is not an image attachment"
            )
        current = (
            await service.app.references.resolve_in(
                tx, ctx, f"{record.target.kind}:{record.target.id}"
            )
        ).reference
        assert isinstance(current, CognitiveTarget)
        previous = await service.app.cognition.get_in(tx, ctx, current.reference)
        exists = record.evidence in previous.evidence
        if exists:
            await service.app.cognition.revise_evidence_in(
                tx, ctx, current.reference, remove=(record.evidence,), reason=command.reason
            )
        return tx.result(
            {
                "success": exists,
                "invalidated": int(exists),
                "error": "" if exists else "Link is already inactive",
            }
        )
