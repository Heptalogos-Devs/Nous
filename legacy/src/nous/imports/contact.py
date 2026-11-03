"""Actual subject contact, distinct from admitting a document or background access."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    BlobDocument,
    ContactMaterial,
    ContactReceipt,
    FormObservation,
)
from pydantic import TypeAdapter
from sqlalchemy import select, update

from nous.identity.access import require_namespace_in
from nous.material.service import MaterialRecord, materials, observations
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous.platform.persistence.authority import canonical_json

if TYPE_CHECKING:
    from .service import ImportService


async def contact(
    self: ImportService, ctx: CallContext, command: ContactMaterial
) -> ContactReceipt:
    # Event identity belongs to the authenticated host and subject. Transport retry
    # operation IDs may change; the reported encounter remains one event.
    submitted = command.model_dump(mode="json", exclude={"operation_id"})
    identity = sha256(
        canonical_json(
            (ctx.caller_id, ctx.namespace_id, ctx.namespace_id, command.external_event_id)
        ).encode()
    ).hexdigest()
    async with self.authority.read(ctx) as view:
        await require_namespace_in(view, ctx, "memory")
        for reference in command.related:
            await self.cognition.get_in(view, ctx, reference)
        source = command.source
        if source.kind in {"representation", "derived_region"}:
            info = await self.cognition.derivations.source_in(view, ctx, source)
            raw = await view.connection.scalar(
                select(materials.c.record).where(
                    materials.c.id == info.material_ids[0],
                    materials.c.namespace_id == ctx.namespace_id,
                )
            )
            material = TypeAdapter(MaterialRecord).validate_python(raw)
        else:
            material, region, event_id = await self.materials.media_source_in(view, ctx, source)
            source = source.model_copy(update={"observation_id": event_id})
    async with self.authority.write(ctx) as tx:
        for reference in command.related:
            await self.cognition.get_in(tx, ctx, reference)
        existing = await tx.connection.scalar(
            select(observations.c.record).where(
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.caller_id == ctx.caller_id,
                observations.c.external_event_id == command.external_event_id,
            )
        )
        if existing is not None:
            if existing.get("purged"):
                raise DomainError("source_unavailable", "Contact source is unavailable")
            if existing.get("submitted_contact") != submitted:
                raise DomainError(
                    "contact_event_conflict", "Contact event has different actual content"
                )
            receipt = ContactReceipt.model_validate(existing["contact_result"])
            await self.materials.accepted_source_in(tx, ctx, receipt.observation_id, receipt.source)
            for reference in receipt.cognition:
                await self.cognition.get_in(tx, ctx, reference)
            return receipt
        # Reauthorize the exact supplied selection in the committing transaction.
        document = BlobDocument(
            filename=material["filename"],
            media_type=material["media_type"],
            source_type="actual_contact",
            source=material.get("source"),
            origin=command.origin,
            observed_at=command.observed_at,
            event_time=command.event_time,
        )
        event = await self.materials.observe_in(
            tx, ctx, material["id"], document, kind="contact", sources=(source,)
        )
        event.update(
            {
                "external_event_id": command.external_event_id,
                "caller_id": ctx.caller_id,
                "contact_source": source.model_dump(mode="json"),
                "participants": list(command.participants),
                "audience": list(command.audience),
                "related_cognition": [
                    reference.model_dump(mode="json") for reference in command.related
                ],
            }
        )
        if command.statement is not None:
            event["statement"] = command.statement
        await tx.connection.execute(
            update(observations)
            .where(
                observations.c.namespace_id == ctx.namespace_id, observations.c.id == event["id"]
            )
            .values(record=event)
        )
        formation = None
        if command.form_long_term:
            formation = await self.submit_form_observation_in(
                tx,
                ctx,
                FormObservation(
                    operation_id=identity + ":form",
                    observation_id=event["id"],
                    source=source,
                    media_strategy=command.media_strategy,
                    background=command.statement or "",
                ),
            )
        from nous.platform.execution import JobSpec

        if self.authority.execution is not None:
            await tx.enqueue(
                JobSpec(
                    workflow_name="nous.maintenance.contact",
                    queue_name="default",
                    workflow_id="contact-growth:" + event["id"],
                    args=(
                        ctx.model_dump(mode="json"),
                        event["id"],
                        [ref.model_dump(mode="json") for ref in command.related],
                    ),
                    scope=ctx.namespace_id,
                )
            )
        result = ContactReceipt(
            observation_id=event["id"],
            namespace_id=ctx.namespace_id,
            caller_id=ctx.caller_id,
            external_event_id=command.external_event_id,
            source=source,
            active_sources=(source,),
            formation=formation,
            commit_seq=tx.commit_seq,
        )
        event["submitted_contact"] = submitted
        event["contact_result"] = result.model_dump(mode="json")
        await tx.connection.execute(
            update(observations)
            .where(
                observations.c.namespace_id == ctx.namespace_id, observations.c.id == event["id"]
            )
            .values(
                record=event, caller_id=ctx.caller_id, external_event_id=command.external_event_id
            )
        )
        return result
