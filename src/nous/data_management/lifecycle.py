"""Coordinate source and cognitive erasure across their actual data owners."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CognitiveRef
from nous.platform.call_context import CallContext

from nous.platform.persistence import WriteTx

if TYPE_CHECKING:
    from nous.cognition.corrections import CorrectionService
    from nous.cognition.motivation_records import MotivationService
    from nous.context.runtime import ContextRuntime
    from nous.data_management.resources import ResourceService
    from nous.feedback.service import FeedbackService
    from nous.maintenance.service import MaintenanceService
    from nous.material.derived import DerivedService
    from nous.platform.persistence import Authority
    from nous.retrieval.tuning import RetrievalTuningService
    from nous.vector_projection.service import VectorProjectionService


class DataLifecycleService:
    def __init__(
        self,
        *,
        authority: Authority,
        contexts: ContextRuntime,
        vector_projection: VectorProjectionService,
        derivations: DerivedService,
        maintenance: MaintenanceService,
        corrections: CorrectionService,
        resources: ResourceService,
        tuning: RetrievalTuningService,
        motivation: MotivationService,
        feedback: FeedbackService,
    ):
        self.authority = authority
        self.contexts = contexts
        self.vector_projection = vector_projection
        self.derivations = derivations
        self.maintenance = maintenance
        self.corrections = corrections
        self.resources = resources
        self.tuning = tuning
        self.motivation = motivation
        self.feedback = feedback

    async def purge_model_inputs_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        await self.vector_projection.purge_materials_in(tx, material_ids)
        await self.derivations.purge_materials_in(tx, material_ids)
        await self.maintenance.purge_materials_in(tx, material_ids)
        await self.corrections.purge_materials_in(tx, material_ids)
        await self.resources.purge_materials_in(tx, material_ids)
        await self.tuning.purge_in(tx, material_ids=material_ids)
        await self.motivation.purge_motivation_assessments_in(tx, material_ids=material_ids)

    async def purge_cognitive_references_in(
        self, tx: WriteTx, ctx: CallContext, references: tuple[CognitiveRef, ...]
    ) -> None:
        await self.contexts.purge_references_in(tx, ctx, references)
        await self.corrections.purge_references_in(tx, ctx, references)
        await self.tuning.purge_in(tx, references=references)
        await self.feedback.purge_references_in(tx, ctx, references)
        await self.motivation.purge_motivation_assessments_in(tx, references=references)

    async def run_cognitive_purge(
        self, context: dict[str, object], references: list[dict[str, object]], operation_id: str
    ) -> dict[str, object]:
        ctx = CallContext.model_validate(context)
        refs = tuple(CognitiveRef.model_validate(value) for value in references)
        async with self.authority.write(ctx) as tx:
            await self.purge_cognitive_references_in(tx, ctx, refs)
        return {"purged": len(refs)}
