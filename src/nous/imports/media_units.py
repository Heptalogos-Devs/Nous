"""A page without a text layer uses the same media interpretation operation."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    AdoptMedia,
    CallContext,
    CognitiveRecord,
    DomainError,
    ImportUnit,
    MediaReference,
    ProcessCatalog,
    UnitOutput,
)
from nous_sdk.contracts.derivation import RepresentationRef
from nous_sdk.contracts.media import MediaStrategy
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.material.service import require_observation_in

from .schema import outputs

if TYPE_CHECKING:
    from .attempts import AttemptRecorder
    from .service import ImportService


async def process_media_unit(
    self: ImportService,
    ctx: CallContext,
    command: ProcessCatalog,
    unit: ImportUnit,
    attempt_job_id: str | None,
    attempt: AttemptRecorder,
) -> UnitOutput:
    if self.media_imports is None or unit.observation_id is None:
        raise DomainError(
            "media_formation_required", "The media interpretation owner is unavailable"
        )
    async with self.authority.read(ctx) as view:
        region = (
            await self.materials.read_region_in(view, ctx, unit.source_region_id)
            if unit.source_region_id
            else None
        )
    source = (
        unit.source
        if unit.source is not None
        else MediaReference(
            kind="source_region", id=unit.source_region_id, observation_id=unit.observation_id
        )
        if unit.source_region_id
        else MediaReference(kind="material", id=unit.material_id, observation_id=unit.observation_id)
    )
    cognition: tuple[CognitiveRecord, ...] = ()
    representations: tuple[RepresentationRef, ...] = ()
    if command.llm_enabled and unit.requires_llm:
        await attempt.advance("formation")
        document = AdoptMedia(
            operation_id=f"{command.operation_id}:{unit.id}:media",
            material_id=unit.material_id,
            observation_id=unit.observation_id,
            source=unit.source,
            regions=() if unit.source is not None else (region,) if region else (),
            media_strategy=TypeAdapter(MediaStrategy).validate_python(
                unit.context.get("media_strategy", "description_only")
            ),
            formation_purpose=command.formation_purpose,
            background=(
                unit.context["background"]
                if isinstance(unit.context.get("background"), str)
                else ""
            ),
        )
        generated = await self.media_imports.adopt(ctx, document)
        cognition = TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            generated["cognition"]
        )
        representations = TypeAdapter(tuple[RepresentationRef, ...]).validate_python(
            generated.get("representations", ())
        )
        await attempt.advance("adoption")

    async with self.authority.write(ctx) as tx:
        saved = await tx.connection.scalar(
            select(outputs.c.record).where(
                outputs.c.unit_id == unit.id,
                outputs.c.operation_id == command.operation_id,
            )
        )
        if saved is not None:
            output = UnitOutput.model_validate(saved)
            await require_observation_in(tx, ctx, output.observation_id)
            for reference in output.cognition:
                await self.cognition.get_in(tx, ctx, reference)
            return output
        await require_observation_in(
            tx, ctx, unit.observation_id, region_id=unit.source_region_id
        )
        cognition_refs = tuple(record.reference for record in cognition)
        targets = (*cognition_refs, *(reference.media() for reference in representations))
        if not targets and command.vectors != "disabled":
            targets = (source,)
        projection = (
            await self.projection.admit_in(tx, ctx, targets)
            if self.projection is not None and command.vectors != "disabled" and targets
            else None
        )
        output = UnitOutput(
            projection=projection,
            id=str(uuid4()),
            unit_id=unit.id,
            observation_id=unit.observation_id,
            cognition=cognition_refs,
            representations=representations,
            attempt_job_id=attempt_job_id,
            vector_policy=command.vectors,
            commit_seq=tx.commit_seq,
        )
        await tx.connection.execute(
            outputs.insert().values(
                id=output.id,
                unit_id=unit.id,
                operation_id=command.operation_id,
                record=output.model_dump(mode="json"),
            )
        )
    attempt.value = attempt.value.model_copy(update={"output_id": output.id})
    return output
