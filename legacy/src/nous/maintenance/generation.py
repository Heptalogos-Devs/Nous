"""Preserve independently validated semantic change groups before adoption."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.common import ModelProvenance
from nous_sdk.contracts.ego_model import SeedProposal
from nous_sdk.contracts.growth import GrowthSnapshot
from nous_sdk.contracts.maintenance import MaintenanceRun
from nous_sdk.contracts.maintenance_tasks import DreamDraft, JournalDraft, ReflectionDraft

from nous.cognition.growth_proposal import growth_model_catalog
from nous.platform.models.generation import MediaInput
from pydantic import JsonValue, TypeAdapter

if TYPE_CHECKING:
    from .service import MaintenanceService


async def generate_validated(
    self: MaintenanceService,
    ctx: CallContext,
    run: MaintenanceRun,
    snapshot: GrowthSnapshot,
) -> tuple[JsonValue, ModelProvenance]:
    from .inputs import materials, range_text

    if run.command.kind == "journal":
        variables: dict[str, object] = {
            "instruction": run.command.instruction,
            "style": "主体日记" if run.command.style == "diary" else "资料学习或工作回顾",
            "materials": materials(snapshot),
            "period": range_text(run.command.period),
        }
    elif run.command.kind in {"reflect", "growth"}:
        variables = {"question": run.command.instruction, "materials": materials(snapshot)}
    elif run.command.kind == "dream":
        variables = {"theme": run.command.instruction, "materials": materials(snapshot)}
    else:
        variables = {
            "maintenance_run_id": run.id,
            "instruction": run.command.instruction,
            "snapshot": growth_model_catalog(snapshot),
        }
    source_ids = tuple(
        dict.fromkeys(
            (
                *snapshot.material_ids,
                *(root for entry in snapshot.entries for root in entry.record.source_roots),
                *(
                    link.observation_id
                    for source in snapshot.sources
                    for link in source.evidence
                    if link.observation_id is not None
                ),
            )
        )
    )
    output_type = {
        "seed": SeedProposal,
        "growth": ReflectionDraft,
        "journal": JournalDraft,
        "reflect": ReflectionDraft,
        "dream": DreamDraft,
    }[run.command.kind]
    role = "reflect" if run.command.kind == "growth" else run.command.kind
    media: list[MediaInput] = []
    for source in snapshot.sources:
        if source.requires_media:
            if self.models.preview_media_resolver is None:
                raise DomainError(
                    "media_unavailable",
                    "The selected material requires its configured media reader",
                )
            media.append(await self.models.preview_media_resolver(ctx, source.source))
    generated = await self.models.generate(
        ctx,
        role,
        variables,
        output_type,
        source_ids=source_ids,
        media=tuple(media),
    )
    return (
        TypeAdapter(JsonValue).validate_python(
            TypeAdapter(object).dump_python(generated.output, mode="json")
        ),
        generated.producer,
    )
