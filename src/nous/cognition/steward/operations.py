"""Arrange explicitly selected material in one model operation."""

import asyncio
import json

from nous.application.contracts import CallContext, LanguageEvents, Progress, define_operation
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import MaterialSelection
from nous.cognition.materials import Materials
from nous.cognition.scope import NamespaceOperations
from nous.common.errors import NousError
from nous.common.operations import add_error_context, record_completed
from nous.content.contracts import MaterialSource, SelectedInputs, SelectedSource
from nous.content.results import ResultStore
from nous.models.generation import GenerationOperations, GenerationRequest

from .contracts import ArrangeReading, ReadingBrief, ReadingOutcome

ARRANGE = define_operation(
    "steward.arrange",
    request=ArrangeReading,
    result=ReadingOutcome,
    events=LanguageEvents,
    description="Arrange selected material for a reading purpose.",
)


class StewardOperations:
    """Arrange supplied exact sources and save the resulting reading."""

    def __init__(
        self,
        generation: GenerationOperations,
        results: ResultStore,
        materials: Materials,
        namespaces: NamespaceOperations,
    ):
        self.generation, self.results, self.materials, self.namespaces = (
            generation,
            results,
            materials,
            namespaces,
        )

    async def arrange(self, request: ArrangeReading, call: CallContext) -> ReadingOutcome:
        from nous.cognition.retrieval.adoption import read_saved_evidence

        if (request.evidence is None) == (not request.materials):
            raise NousError("input.invalid", "Choose saved evidence or material selections")
        if request.evidence is None:
            namespace = await self.namespaces.resolve_for(request.namespace)
            selections: list[MaterialSelection] = []
            for item in request.materials:
                selections.append(
                    item
                    if isinstance(item, MaterialSelection)
                    else MaterialSelection(
                        material=await self.materials.resolve_identifier(item, namespace=namespace)
                    )
                )
            resolved = await self.materials.selections(selections, namespace=namespace)
            sources = tuple(
                MaterialSource(
                    key=f"E{index}", material=view.ref, selections=ranges, name=view.title
                )
                for index, (view, ranges) in enumerate(resolved, 1)
            )
            return await self.arrange_sources(
                request.model_copy(update={"namespace": namespace}), sources, call
            )
        evidence = await read_saved_evidence(
            self.results, request.evidence, include_context=request.context is None
        )
        namespace = await self.namespaces.resolve_for(request.namespace, evidence.namespace)
        context = request.context if request.context is not None else evidence.context
        prepared = request.model_copy(
            update={
                "evidence": evidence.ref,
                "namespace": namespace,
                "question": request.question if request.question is not None else evidence.question,
                "aspects": request.aspects if request.aspects is not None else evidence.aspects,
                "context": context,
            }
        )
        return await self.arrange_sources(prepared, evidence.sources, call)

    async def arrange_sources(
        self, request: ArrangeReading, sources: tuple[MaterialSource, ...], call: CallContext
    ) -> ReadingOutcome:
        async with self.generation.execution.operation("steward.arrange"):
            evidence_ref = request.evidence
            namespace = request.namespace
            if not isinstance(namespace, NamespaceRef) or isinstance(evidence_ref, str):
                raise NousError("input.invalid", "Arrange requires resolved material context")
            if not sources or len({source.key for source in sources}) != len(sources):
                raise NousError(
                    "steward.invalid_evidence", "Source keys must be nonempty and unique"
                )
            if evidence_ref is not None:
                record_completed(evidence_ref)
            try:
                task = request.task or "arrange-reading"
                self.generation.require_contract(task, "arrange-reading", call.settings)
                instructions = json.dumps(
                    {
                        "purpose": request.purpose,
                        "detail": request.detail,
                        "output_language": request.output_language,
                        "aspects": list(request.aspects or ()),
                        "context": list(request.context or ()),
                        "evidence": [{"key": item.key, "title": item.name} for item in sources],
                    },
                    ensure_ascii=False,
                    indent=2,
                )
                generation = GenerationRequest(
                    task=task,
                    inputs=SelectedInputs(
                        sources=tuple(
                            SelectedSource(
                                key=item.key,
                                material=item.material,
                                selections=item.selections,
                                kind="source",
                                name=item.name,
                            )
                            for item in sources
                        )
                    ),
                    question=request.question if request.question is not None else "",
                    instructions=instructions,
                )
                await call.events.emit(Progress(stage="arrange"))
                completed = await self.generation.compute(generation, call)
                brief = completed.value
                if not isinstance(brief, ReadingBrief):
                    raise NousError("steward.invalid_output", "Reading task returned no brief")
                formal_sources = completed.facts.material_sources
                dependencies = completed.facts.dependencies
                if evidence_ref is not None:
                    dependencies = tuple(dict.fromkeys((*dependencies, evidence_ref)))
                facts = completed.facts.model_copy(
                    update={
                        "dependencies": dependencies,
                    }
                )
                await call.events.emit(Progress(stage="save_reading"))
                prepared = await self.results.prepare_outputs(completed.outputs)
                async with self.results.database.write() as tx:
                    await self.materials.validate_sources_in(
                        tx, formal_sources, namespace=namespace
                    )
                    saved = await self.results.adopt_in(tx, facts=facts, prepared_outputs=prepared)
                await call.saved(saved.ref)
                return ReadingOutcome(evidence_ref=evidence_ref, reading_ref=saved.ref, brief=brief)
            except (NousError, ValueError, asyncio.CancelledError, TimeoutError) as error:
                if evidence_ref is not None:
                    add_error_context(
                        error,
                        {
                            "evidence_ref": evidence_ref.model_dump(mode="json"),
                            "completed_refs": [evidence_ref.model_dump(mode="json")],
                        },
                    )
                raise
