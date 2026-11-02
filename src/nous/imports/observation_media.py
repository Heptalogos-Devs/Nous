"""Accepted media encounters adopt model interpretations with their source provenance."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import AdoptMedia, CallContext, CognitiveRecord, DomainError
from nous_sdk.contracts.derivation import RepresentationRef
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
    async with self.authority.read(ctx) as view:
        selected = await self.materials.accepted_source_in(
            view, ctx, command.observation_id, command.source
        )
    generated = await self.media_imports.adopt(
        ctx,
        AdoptMedia(
            operation_id=command.operation_id + ":media",
            observation_id=command.observation_id,
            material_id=selected.material["id"],
            source=selected.source,
            media_strategy=command.media_strategy,
            formation_purpose=command.formation_purpose,
            background=command.background,
        ),
    )
    records = TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(generated["cognition"])
    refs = tuple(record.reference for record in records)
    representations = TypeAdapter(tuple[RepresentationRef, ...]).validate_python(
        generated.get("representations", ())
    )
    targets = (
        *refs,
        *(reference.media() for reference in representations),
    )
    async with self.authority.write(ctx) as tx:
        from .observation import formation_result_in, save_formation_in

        saved = await formation_result_in(self, tx, ctx, command)
        if saved is not None:
            return saved
        await self.materials.accepted_source_in(tx, ctx, command.observation_id, command.source)
        for reference in refs:
            await self.cognition.get_in(tx, ctx, reference)
        return await save_formation_in(
            self,
            tx,
            ctx,
            command,
            ObservationFormation(
                observation_id=command.observation_id,
                cognition=refs,
                representations=representations,
                projection=await self.projection.admit_in(tx, ctx, targets)
                if self.projection is not None and command.vectors != "disabled" and targets
                else None,
                commit_seq=tx.commit_seq,
            ),
        )
