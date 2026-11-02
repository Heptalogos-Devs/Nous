"""Coordinate source and cognitive erasure across their actual data owners."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRef

from nous.platform.persistence import WriteTx

if TYPE_CHECKING:
    from nous.cognition.corrections import CorrectionService
    from nous.cognition.motivation_records import MotivationService
    from nous.context.service import ContextService
    from nous.data_management.resources import ResourceService
    from nous.feedback.service import FeedbackService
    from nous.maintenance.service import MaintenanceService
    from nous.material.derived import DerivedService
    from nous.material.description_deliveries import DescriptionDeliveryService
    from nous.platform.models import ModelService
    from nous.platform.persistence import Authority
    from nous.retrieval.tuning import RetrievalTuningService
    from nous.symbols.service import SymbolsService
    from nous.vector_projection.service import VectorProjectionService


class DataLifecycleService:
    def __init__(
        self,
        *,
        authority: Authority,
        contexts: ContextService,
        description_deliveries: DescriptionDeliveryService,
        vector_projection: VectorProjectionService,
        models: ModelService,
        derivations: DerivedService,
        maintenance: MaintenanceService,
        corrections: CorrectionService,
        resources: ResourceService,
        symbols: SymbolsService,
        tuning: RetrievalTuningService,
        motivation: MotivationService,
        feedback: FeedbackService,
    ):
        self.authority = authority
        self.contexts = contexts
        self.description_deliveries = description_deliveries
        self.vector_projection = vector_projection
        self.models = models
        self.derivations = derivations
        self.maintenance = maintenance
        self.corrections = corrections
        self.resources = resources
        self.symbols = symbols
        self.tuning = tuning
        self.motivation = motivation
        self.feedback = feedback

    async def purge_model_inputs_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        await self.description_deliveries.purge_in(tx, material_ids=material_ids)
        await self.vector_projection.purge_materials_in(tx, material_ids)
        await self.models.ledger.purge_materials_in(tx, material_ids)
        await self.derivations.purge_materials_in(tx, material_ids)
        await self.maintenance.purge_materials_in(tx, material_ids)
        await self.corrections.purge_materials_in(tx, material_ids)
        await self.resources.purge_materials_in(tx, material_ids)
        await self.symbols.purge_references_in(tx, tx.ctx, material_ids)
        await self.tuning.purge_in(tx, material_ids=material_ids)
        await self.motivation.purge_motivation_assessments_in(tx, material_ids=material_ids)

    async def purge_cognitive_references_in(
        self, tx: WriteTx, ctx: CallContext, references: tuple[CognitiveRef, ...]
    ) -> None:
        await self.contexts.purge_references_in(tx, ctx, references)
        await self.description_deliveries.purge_in(tx, references=references)
        await self.corrections.purge_references_in(tx, ctx, references)
        await self.tuning.purge_in(tx, references=references)
        await self.feedback.purge_references_in(tx, ctx, references)
        await self.symbols.purge_references_in(tx, ctx, (reference.id for reference in references))
        await self.motivation.purge_motivation_assessments_in(tx, references=references)

    async def run_cognitive_purge(
        self, context: dict[str, object], references: list[dict[str, object]], operation_id: str
    ) -> dict[str, object]:
        ctx = CallContext.model_validate(context)
        refs = tuple(CognitiveRef.model_validate(value) for value in references)
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="cognition.purge_dependents",
            scope=ctx.namespace_id,
            payload={"references": refs},
        ) as tx:
            if not tx.replayed:
                await self.purge_cognitive_references_in(tx, ctx, refs)
                tx.result({"purged": len(refs)})
        return {"purged": len(refs)}
