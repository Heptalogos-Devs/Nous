"""A document page without a text layer uses the existing media interpretation path."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal
from uuid import uuid4

from nous_sdk.contracts import (
    AdoptMedia,
    CallContext,
    CognitiveRecord,
    DomainError,
    ImportUnit,
    ProcessCatalog,
    UnitOutput,
)
from pydantic import TypeAdapter

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
    if not command.llm_enabled:
        raise DomainError(
            "pdf_page_requires_ocr",
            "The selected page has no text layer; enable media interpretation",
        )
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
    await attempt.advance("formation")
    generated = await self.media_imports.adopt(
        ctx,
        AdoptMedia(
            operation_id=f"{command.operation_id}:{unit.id}:media",
            material_id=unit.material_id,
            observation_id=unit.observation_id,
            regions=(region,) if region else (),
            strategy=TypeAdapter(
                Literal["native", "description", "description_then_native"]
            ).validate_python(unit.context.get("media_strategy", "description")),
            instruction="Read and transcribe the selected content, preserving headings, tables, visible text, speech and meaningful visual actions. Describe interpretation separately from the observed content.",
        ),
    )
    records = TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(generated["cognition"])
    await attempt.advance("adoption")
    async with self.authority.write(
        ctx,
        f"{command.operation_id}:{unit.id}:adopt",
        operation_kind="imports.adopt",
        scope=ctx.namespace_id,
        payload={"command": command.model_dump(mode="json"), "unit_id": unit.id},
    ) as tx:
        if tx.replayed:
            return UnitOutput.model_validate(tx.replay_result)
        await require_observation_in(tx, ctx, unit.observation_id, region_id=unit.source_region_id)
        output = UnitOutput(
            projection=await self.projection.admit_in(
                tx, ctx, tuple(record.reference for record in records)
            )
            if self.projection is not None and command.vectors != "disabled"
            else None,
            id=str(uuid4()),
            unit_id=unit.id,
            observation_id=unit.observation_id,
            cognition=tuple(record.reference for record in records),
            attempt_job_id=attempt_job_id,
            producer_call_id=TypeAdapter(str | None).validate_python(
                generated.get("producer_call_id")
            ),
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
        return tx.result(output)
