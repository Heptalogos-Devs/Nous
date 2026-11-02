"""Actual subject contact, distinct from admitting a document or background access."""

from __future__ import annotations

import asyncio
from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    BlobDocument,
    CallContext,
    CognitiveTimes,
    ContactMaterial,
    ContactReceipt,
    DomainError,
    FormObservation,
    MemoryBody,
)
from nous_sdk.contracts.derivation import StructuredRepresentation, TextRepresentation
from pydantic import TypeAdapter
from sqlalchemy import select, update

from nous.identity.access import require_namespace_in
from nous.material.service import MaterialRecord, materials, observations
from nous.material.text import source_region_text
from nous.platform.content import ContentObject
from nous.platform.persistence.authority import canonical_json

if TYPE_CHECKING:
    from .service import ImportService


async def contact(
    self: ImportService, ctx: CallContext, command: ContactMaterial
) -> ContactReceipt:
    # Event identity belongs to the authenticated host and subject. Transport retry
    # operation IDs may change; the reported encounter remains one event.
    identity = sha256(
        canonical_json(
            (ctx.caller_id, ctx.namespace_id, ctx.namespace_id, command.external_event_id)
        ).encode()
    ).hexdigest()
    async with self.authority.read(ctx) as view:
        await require_namespace_in(view, ctx, "memory")
        for reference in command.related:
            await self.cognition.get_in(view, ctx, reference)
        text: str | None = None
        source = command.source
        if source.kind in {"representation", "derived_region"}:
            info = await self.cognition.derivations.source_in(view, ctx, source)
            selected = await self.cognition.derivations.selected_in(view, ctx, source)
            if isinstance(selected.payload, TextRepresentation):
                text = selected.payload.text
            elif isinstance(selected.payload, StructuredRepresentation):
                text = canonical_json(selected.payload.value)
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
            text = material["text"]
            if text is None and (
                material["media_type"].startswith("text/") or "json" in material["media_type"]
            ):
                if self.content_store is None:
                    raise DomainError(
                        "content_unavailable", "The original text bytes are unavailable"
                    )
                async with self.content_store.protect(
                    ContentObject(
                        material["content_hash"],
                        material["byte_len"],
                        material["media_type"],
                    )
                ) as path:
                    text = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
            if text is not None and region is not None:
                text = source_region_text(text, region)
        if text is None and command.statement is not None:
            text = command.statement
    async with self.authority.write(
        ctx,
        identity,
        operation_kind="material.contact",
        scope=ctx.namespace_id,
        payload=command.identity_payload(),
    ) as tx:
        for reference in command.related:
            await self.cognition.get_in(tx, ctx, reference)
        if tx.replayed:
            receipt = ContactReceipt.model_validate(tx.replay_result)
            await self.materials.accepted_source_in(
                tx,
                ctx,
                receipt.observation_id,
                receipt.source,
                derivations=self.cognition.derivations,
            )
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
            update(observations).where(observations.c.id == event["id"]).values(record=event)
        )
        refs = ()
        if text:
            # Encounters do not turn repeated reads of one original into independent evidence.
            record = await self.cognition.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    body=MemoryBody(
                        text=text,
                        media=(source,),
                        role="experiential",
                        external_ids={"contact": command.external_event_id},
                        contact_observation_id=event["id"],
                    ),
                    evidence=(),
                    times=CognitiveTimes(
                        event=command.event_time,
                        observation=command.observed_at or ctx.clock,
                        formation=ctx.clock,
                        recorded=ctx.clock,
                    ),
                ),
            )
            refs = (record.reference,)
        formation = None
        if command.form_long_term:
            formation = await self.submit_form_observation_in(
                tx,
                ctx,
                FormObservation(
                    operation_id=identity + ":form",
                    observation_id=event["id"],
                    source=source,
                    anchor=refs[0] if refs else None,
                    media_strategy=command.media_strategy,
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
                        [
                            ref.model_dump(mode="json")
                            for ref in dict.fromkeys((*refs, *command.related))
                        ],
                    ),
                )
            )
        return tx.result(
            ContactReceipt(
                observation_id=event["id"],
                namespace_id=ctx.namespace_id,
                caller_id=ctx.caller_id,
                external_event_id=command.external_event_id,
                source=source,
                active_sources=(source,),
                cognition=refs,
                formation=formation,
                commit_seq=tx.commit_seq,
            )
        )
