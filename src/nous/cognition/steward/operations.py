"""Arrange a saved evidence delivery without repeating its retrieval."""

import asyncio
import json

from nous.application.contracts import CallContext, LanguageEvents, Progress, define_operation
from nous.cognition.material_contracts import MaterialRef, MaterialSelection, MaterialSource
from nous.cognition.materials import Materials
from nous.cognition.retrieval.contracts import EvidenceDelivery, QueryControls
from nous.common.errors import NousError
from nous.common.operations import add_error_context, record_completed
from nous.content.contracts import ContentSelection, ResultOutputRef, SelectedInputs, SelectedSource
from nous.content.results import ResultStore
from nous.models.generation import GenerationOperations, GenerationRequest

from .contracts import ArrangeReading, ReadingBrief, ReadingOutcome

ARRANGE = define_operation(
    "steward.arrange",
    request=ArrangeReading,
    result=ReadingOutcome,
    events=LanguageEvents,
    description="Arrange a saved evidence delivery for a reading purpose.",
)


class StewardOperations:
    """Generate and adopt an independently reusable reading result."""

    def __init__(
        self, generation: GenerationOperations, results: ResultStore, materials: Materials
    ) -> None:
        self.generation, self.results, self.materials = generation, results, materials

    @staticmethod
    def _overlaps(left: ContentSelection, right: ContentSelection) -> bool:
        if left.artifact != right.artifact:
            return False
        if left.selection.is_whole or right.selection.is_whole:
            return True
        start, end = left.selection.start_byte, left.selection.end_byte
        chosen_start, chosen_end = right.selection.start_byte, right.selection.end_byte
        return (
            start is not None
            and end is not None
            and chosen_start is not None
            and chosen_end is not None
            and start < chosen_end
            and chosen_start < end
        )

    async def arrange(self, request: ArrangeReading, call: CallContext) -> ReadingOutcome:
        async with self.generation.execution.operation("steward.arrange"):
            record_completed(request.evidence)
            try:
                artifact = await self.results.resolve_output(
                    ResultOutputRef(result=request.evidence, output="evidence")
                )
                delivery = EvidenceDelivery.model_validate_json(
                    await self.results.artifacts.read_text(artifact)
                )
                keys = [passage.key for passage in delivery.passages]
                if len(keys) != len(set(keys)):
                    raise NousError("steward.invalid_evidence", "Evidence keys must be unique")
                if not keys:
                    raise NousError(
                        "steward.empty_evidence", "The delivery has no passages to arrange"
                    )
                materials = {item.material.ref: item.material for item in delivery.materials}
                sources: list[SelectedSource] = []
                catalogue: list[dict[str, object]] = []
                for passage in delivery.passages:
                    material = materials.get(passage.material)
                    selections = passage.selections
                    if not selections:
                        raise NousError(
                            "steward.invalid_evidence",
                            "An evidence item has no readable selections",
                        )
                    normalized = await self.results.artifacts.normalize_selections(selections)
                    title = material.title if material else None
                    sources.append(
                        SelectedSource(
                            slot_key=passage.key,
                            selections=selections,
                            display_name=f"{passage.key}: {title or 'Evidence'}",
                        )
                    )
                    catalogue.append(
                        {
                            "key": passage.key,
                            "title": title,
                            "times": material.times.model_dump(mode="json")
                            if material and material.times
                            else None,
                            "heading_paths": list(
                                dict.fromkeys(
                                    section.heading_path
                                    for section in material.sections
                                    if any(
                                        self._overlaps(section.selection, selection)
                                        for selection in normalized
                                    )
                                )
                            )
                            if material
                            else [],
                        }
                    )

                def controls(value: QueryControls) -> dict[str, object]:
                    return value.model_dump(
                        mode="json",
                        exclude_none=True,
                        exclude_defaults=True,
                        include={
                            "selectors",
                            "limit",
                            "return_types",
                            "times",
                            "time_clauses",
                            "prefer",
                            "avoid",
                            "sort",
                            "explore",
                            "include_review",
                            "include_exploration",
                        },
                    )

                def identifier(value: MaterialRef) -> str:
                    if isinstance(value, ResultOutputRef):
                        return f"{value.result.identifier}:{value.output}"
                    return value.identifier

                instructions = json.dumps(
                    {
                        "purpose": request.purpose,
                        "detail": request.detail,
                        "output_language": request.output_language,
                        "aspects": list(delivery.intent.aspects),
                        "constraints": {
                            "controls": controls(delivery.intent.constraints.controls),
                            "branches": [
                                {
                                    "path": list(branch.path),
                                    "controls": controls(branch.controls),
                                    "materials": [identifier(item) for item in branch.materials],
                                    "concepts": [item.identifier for item in branch.concepts],
                                }
                                for branch in delivery.intent.constraints.branches
                            ],
                            "materials": [
                                identifier(item) for item in delivery.intent.bound_materials
                            ],
                            "concepts": [
                                item.identifier for item in delivery.intent.bound_concepts
                            ],
                        },
                        "context": [
                            item.model_dump(mode="json") for item in delivery.intent.context
                        ],
                        "evidence": catalogue,
                    },
                    ensure_ascii=False,
                    indent=2,
                )
                task = request.task or "arrange-reading"
                self.generation.require_contract(task, "arrange-reading", call.settings)
                await call.events.emit(Progress(stage="arrange"))
                completed = await self.generation.compute(
                    GenerationRequest(
                        task=task,
                        inputs=SelectedInputs(sources=tuple(sources)),
                        question=delivery.intent.original_text,
                        instructions=instructions,
                    ),
                    call,
                )
                output = completed.outputs.get("reading")
                if output is None:
                    raise NousError("steward.invalid_output", "Reading task returned no brief")
                brief = ReadingBrief.model_validate_json(output.data)
                selections = {item.key: item.sources for item in completed.facts.selections}
                formal_sources: list[MaterialSource] = []
                for passage in delivery.passages:
                    used = selections.get(passage.key, ())
                    if not used:
                        continue
                    material = materials.get(passage.material)
                    formal_sources.append(
                        MaterialSource(
                            key=passage.key,
                            material=passage.material,
                            selections=used,
                            name=material.title if material else None,
                        )
                    )
                await self.materials.read_many(
                    tuple(
                        MaterialSelection(material=item.material, selections=item.selections)
                        for item in formal_sources
                    ),
                    namespace=delivery.intent.constraints.namespace,
                )
                facts = completed.facts.model_copy(
                    update={
                        "dependencies": tuple(
                            dict.fromkeys((*completed.facts.dependencies, request.evidence))
                        ),
                    }
                )
                await call.events.emit(Progress(stage="save_reading"))
                prepared = await self.results.prepare_outputs(completed.outputs)
                async with self.results.database.write() as tx:
                    saved = await self.results.adopt_in(tx, facts=facts, prepared_outputs=prepared)
                    await self.materials.adopt_sources_in(tx, saved, "reading", formal_sources)
                    await self.materials.adopt_sources_in(tx, saved, "markdown", formal_sources)
                await call.saved(saved.ref)
                return ReadingOutcome(
                    evidence_ref=request.evidence, reading_ref=saved.ref, brief=brief
                )
            except (NousError, ValueError, asyncio.CancelledError, TimeoutError) as error:
                add_error_context(
                    error, {"completed_refs": [request.evidence.model_dump(mode="json")]}
                )
                raise
