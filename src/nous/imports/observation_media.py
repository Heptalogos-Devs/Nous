"""Accepted media encounters use the existing paid media formation and durable receipts."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import AdoptMedia, CallContext, CognitiveRecord, DomainError
from nous_sdk.contracts.imports import FormObservation, ObservationFormation
from pydantic import TypeAdapter

if TYPE_CHECKING:
    from .service import ImportService


async def form_media_observation(
    self: ImportService,
    ctx: CallContext,
    command: FormObservation,
) -> ObservationFormation:
    if not command.llm_enabled or self.media_imports is None:
        raise DomainError(
            "media_formation_required", "Media formation requires a configured media model"
        )
    if command.anchor is not None:
        raise DomainError(
            "media_anchor_mismatch",
            "Use the retained media interpretation to select an existing text anchor",
        )
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="cognition.form_observation",
        scope=ctx.namespace_id,
        payload=command.identity_payload(),
    )
    if receipt is not None:
        return ObservationFormation.model_validate(receipt.result)
    async with self.authority.read(ctx) as view:
        selected = await self.materials.accepted_source_in(
            view,
            ctx,
            command.observation_id,
            command.source,
            derivations=self.cognition.derivations,
        )
    generated = await self.media_imports.adopt(
        ctx,
        AdoptMedia(
            operation_id=command.operation_id + ":media",
            observation_id=command.observation_id,
            material_id=selected.material["id"],
            source=selected.source,
            strategy=command.media_strategy,
        ),
    )
    records = TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(generated["cognition"])
    refs = tuple(record.reference for record in records)
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="cognition.form_observation",
        scope=ctx.namespace_id,
        payload=command.identity_payload(),
    ) as tx:
        if tx.replayed:
            return ObservationFormation.model_validate(tx.replay_result)
        current = await self.materials.accepted_source_in(
            tx, ctx, command.observation_id, command.source, derivations=self.cognition.derivations
        )
        for reference in refs:
            await self.cognition.get_in(tx, ctx, reference)
        if current.event["kind"] == "contact" and self.on_contact_in is not None:
            await self.on_contact_in(tx, ctx, command.observation_id, refs)
        return tx.result(
            ObservationFormation(
                observation_id=command.observation_id,
                cognition=refs,
                producer_call_id=TypeAdapter(str | None).validate_python(
                    generated.get("producer_call_id")
                ),
                projection=await self.projection.admit_in(tx, ctx, refs)
                if self.projection is not None and command.vectors != "disabled"
                else None,
                commit_seq=tx.commit_seq,
            )
        )
