"""Preserve independently validated semantic change groups before adoption."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.ego_model import SeedProposal
from nous_sdk.contracts.growth import GrowthDraft, GrowthSnapshot
from nous_sdk.contracts.maintenance import CaptureMaintenanceOutput, MaintenanceRun
from nous_sdk.contracts.maintenance_tasks import DreamDraft, JournalDraft, ReflectionDraft

from nous.cognition.growth_proposal import growth_model_catalog
from nous.platform.errors import NousError
from nous.platform.models.generation import MediaInput

if TYPE_CHECKING:
    from .service import MaintenanceService


async def generate_validated(
    self: MaintenanceService,
    ctx: CallContext,
    run: MaintenanceRun,
    snapshot: GrowthSnapshot,
) -> tuple[GrowthDraft, str]:
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
    try:
        generated = await self.models.generate(
            ctx,
            role,
            variables,
            output_type,
            source_ids=source_ids,
            invocation_id="maintenance:" + run.id,
            media=tuple(media),
        )
        call_id = generated.call_id
    except (DomainError, NousError) as error:
        attempt = await self.models.ledger.attempt(ctx, role, "maintenance:" + run.id)
        details = error.details if isinstance(error, DomainError) else error.context
        failed_call = details.get("call_id")
        call_id = attempt.call_id if attempt is not None else failed_call
        if not isinstance(call_id, str):
            raise
        # A routed failure is retained against its physical child invocation;
        # the successful-parent binding does not exist yet. Preserve its actual
        # response for review without retrying the provider or changing ledger facts.
        retained = await self.models.retained_output(ctx, call_id)
        if retained.raw_text is None and retained.saved_output is None:
            raise
    current = await self.read(ctx, run.id)
    captured = await self.capture_retained(
        ctx,
        run.id,
        CaptureMaintenanceOutput(
            operation_id=run.id + ":capture:" + call_id,
            expected_revision=current.revision,
            call_id=call_id,
        ),
    )
    return captured.draft or GrowthDraft(
        summary="Retained proposal requires editing",
        rationale="See individual action diagnostics",
        actions=(),
    ), call_id
