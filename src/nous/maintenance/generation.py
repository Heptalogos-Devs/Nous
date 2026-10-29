"""Validate semantic proposals before publication, with one explicit correction call."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.growth import GrowthDraft, GrowthProposal, GrowthSnapshot
from nous_wave.contracts.maintenance import MaintenanceRun

from nous.cognition.growth_proposal import growth_model_catalog, normalize_growth

if TYPE_CHECKING:
    from .service import MaintenanceService

# Only known proposal semantics authorize another paid call. Source withdrawal,
# stale targets, permission failures, provider errors and unknown outcomes do not.
_CORRECTABLE = frozenset(
    {
        "growth_key_conflict",
        "growth_key_unknown",
        "growth_target_unknown",
        "growth_binding_required",
        "growth_binding_invalid",
        "growth_body_invalid",
        "growth_reference_unknown",
        "growth_media_reference_unknown",
        "growth_structural_action_required",
        "cognitive_kind_immutable",
        "identity_conflict",
        "memory_semantics_anchor",
        "journal_episode_required",
        "support_required",
        "episode_order",
        "social_entity_required",
        "social_group_required",
        "social_trust_invalid",
        "language_audience_required",
        "language_audience_invalid",
        "episode_parent_invalid",
        "knowledge_cutoff",
        "concept_merge",
        "concept_split",
        "concept_partition",
    }
)


async def generate_validated(
    self: MaintenanceService,
    ctx: CallContext,
    run: MaintenanceRun,
    snapshot: GrowthSnapshot,
) -> tuple[GrowthDraft, str]:
    variables: dict[str, object] = {
        "maintenance_run_id": run.id,
        "instruction": run.command.instruction,
        "snapshot": growth_model_catalog(snapshot),
        "body_schema": GrowthProposal.model_json_schema(),
    }
    source_ids = tuple(
        dict.fromkeys(
            (
                *snapshot.material_ids,
                *(root for entry in snapshot.entries for root in entry.record.source_roots),
            )
        )
    )
    for attempt in range(2):
        invocation = "maintenance:" + run.id + (":correction:1" if attempt else "")
        generated = await self.models.generate(
            ctx,
            "maintenance",
            variables,
            GrowthProposal,
            source_ids=source_ids,
            invocation_id=invocation,
        )
        await self.record_generation_call(ctx, run.id, generated.call_id)
        draft = normalize_growth(generated.output)
        try:
            # ReadView enforces query_only at the connection, so the domain's
            # validation cannot allocate identities or write cognitive facts.
            async with self.authority.read(ctx) as view:
                await self.cognition.validate_growth_in(view, ctx, draft, snapshot)
        except DomainError as error:
            if attempt or error.code not in _CORRECTABLE:
                raise
            variables = {
                **variables,
                "previous_proposal": generated.output.model_dump(mode="json"),
                "validation_error": {
                    "code": error.code,
                    "message": str(error),
                    "context": error.details,
                },
            }
        else:
            return draft, generated.call_id
    raise RuntimeError("A maintenance proposal must pass domain validation before publication")
