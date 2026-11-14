"""Describe and structure media from the same explicit original selections."""

from __future__ import annotations

import asyncio
from typing import Literal

from pydantic import Field, JsonValue, model_validator

from nous.application.contracts import CallContext, LanguageEvents, define_operation
from nous.common.errors import NousError
from nous.common.operations import add_error_context
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    ContentValue,
    ExternalInput,
    ResultOutputRef,
    ResultRef,
    SelectedSource,
    SourceFacts,
    SourceSelection,
    SourceTarget,
)
from nous.content.results import ResultStore
from nous.models.generation import GenerationOperations, GenerationRequest
from nous.models.input import semantic_units
from nous.runtime.execution import ExecutionManager

from .preparation import MediaPreparer


class AnalyzeMedia(ContentValue):
    source: SourceTarget
    strategy: Literal["description_only", "direct_structured", "describe_then_structure"]
    question: str = ""
    instructions: str = ""
    description_task: str | None = None
    structure_task: str | None = None
    description: ResultOutputRef | ResultRef | str | None = None
    document_mode: Literal["auto", "text", "visual"] = "auto"
    input_mode: Literal["joint", "independent"] = "joint"
    independent_ranges: tuple[SourceSelection, ...] = ()
    output_schema: dict[str, JsonValue] | ExternalInput | None = None
    without_audio: bool = False

    @model_validator(mode="after")
    def valid_strategy(self) -> AnalyzeMedia:
        if self.independent_ranges and self.input_mode != "independent":
            raise ValueError("Independent ranges require independent input mode")
        if self.description is not None and self.strategy != "describe_then_structure":
            raise ValueError("A description is used by describe-then-structure")
        if self.description is not None and len(self.independent_ranges) > 1:
            raise ValueError("One description applies to one selected range")
        return self


class AnalysisOutcome(ContentValue):
    original: ArtifactRef
    descriptions: tuple[ResultRef, ...] = ()
    results: tuple[ResultRef, ...] = ()
    coverage: dict[str, JsonValue] = Field(default_factory=dict)


class InspectMedia(ContentValue):
    source: SourceTarget


class MediaInspection(ContentValue):
    artifact: Artifact
    facts: SourceFacts


ANALYZE_MEDIA = define_operation(
    "media.analyze",
    request=AnalyzeMedia,
    result=AnalysisOutcome,
    events=LanguageEvents,
    description="Describe or structure a selected media source.",
)
INSPECT_MEDIA = define_operation(
    "media.inspect",
    request=InspectMedia,
    result=MediaInspection,
    events=LanguageEvents,
    description="Inspect a media source and its detected format.",
)


class MediaAnalysis:
    """Save each completed description or structured result before continuing."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        preparer: MediaPreparer,
        generation: GenerationOperations,
        results: ResultStore,
        execution: ExecutionManager,
    ) -> None:
        self.artifacts, self.preparer, self.generation = artifacts, preparer, generation
        self.results, self.execution = results, execution

    async def inspect(self, request: InspectMedia, call: CallContext) -> MediaInspection:
        selected = await self.generation.preparation.accept_targets((request.source,), call)
        artifact = await self.artifacts.get(selected.sources[0].selections[0].artifact)
        return MediaInspection(artifact=artifact, facts=selected.facts[0])

    async def analyze(self, request: AnalyzeMedia, call: CallContext) -> AnalysisOutcome:
        output_schema = await self.generation.resolve_schema(request.output_schema, call)
        selected = await self.generation.preparation.accept_targets((request.source,), call)
        source = selected.sources[0]
        original = source.selections[0]
        settings = call.settings.values.media.analysis
        description_task = request.description_task or settings.description_task
        structure_task = request.structure_task or settings.extraction_task
        unit_task = (
            structure_task
            if request.strategy == "direct_structured" or request.description is not None
            else description_task
        )
        task = self.generation.executor.freeze(unit_task, call.settings)
        if task.resource is None:
            raise NousError("model.task_contract_missing", "The task has no output contract")
        contract = self.generation.executor.compiler.catalog.tasks[
            task.resource.manifest.contract_id
        ]
        units = semantic_units(
            selected, request.input_mode, request.independent_ranges, contract=contract
        )
        if request.description is not None and len(units) != 1:
            raise NousError("media.description_mismatch", "One description applies to one range")
        ranges = tuple(unit.inputs.sources[0].selections[0].selection for unit in units)
        descriptions: list[ResultRef] = []
        results: list[ResultRef] = []
        coverage: list[JsonValue] = []
        failed_stage: Literal["description", "structure"] = "description"
        try:
            for unit in units:
                scope = unit.inputs
                failed_stage = (
                    "structure" if request.strategy == "direct_structured" else "description"
                )
                description_ref: ResultRef | None = None
                description_output: str | None = None
                if request.description is not None:
                    requested = request.description
                    saved = await self.results.get(
                        requested.result if isinstance(requested, ResultOutputRef) else requested
                    )
                    candidates = tuple(
                        item
                        for item in saved.outputs
                        if item.representation_kind == "description"
                        and (
                            not isinstance(requested, ResultOutputRef)
                            or item.name == requested.output
                        )
                    )
                    if len(candidates) != 1:
                        raise NousError(
                            "media.description_mismatch", "Choose one description output"
                        )
                    description_output = candidates[0].name
                    related = tuple(
                        origin
                        for mapping in saved.mappings
                        if mapping.output_name == description_output
                        for origin in mapping.origins
                    )
                    if not any(item.artifact == original.artifact for item in related):
                        raise NousError(
                            "media.description_mismatch", "Description refers to another source"
                        )
                    description_ref = saved.ref
                    descriptions.append(saved.ref)
                elif request.strategy != "direct_structured":
                    generated = await self.generation.compute(
                        GenerationRequest(
                            task=description_task,
                            inputs=scope,
                            question=request.question,
                            instructions=request.instructions,
                            input_mode=unit.mode,
                            document_mode=request.document_mode,
                            without_audio=request.without_audio,
                        ),
                        call,
                    )
                    primary = [
                        name for name, value in generated.outputs.items() if value.role == "primary"
                    ]
                    if len(primary) != 1:
                        raise NousError(
                            "media.description_mismatch", "Description must have one primary output"
                        )
                    outputs = {
                        name: value.model_copy(update={"representation_kind": "description"})
                        if name == primary[0]
                        else value
                        for name, value in generated.outputs.items()
                    }
                    saved = await self.results.adopt(facts=generated.facts, outputs=outputs)
                    description_ref = saved.ref
                    description_output = primary[0]
                    descriptions.append(saved.ref)
                    await call.saved(saved.ref)
                    if request.strategy == "description_only":
                        coverage.append(saved.coverage)
                        continue
                structured_inputs = scope
                failed_stage = "structure"
                if description_ref is not None:
                    description = await self.results.get(description_ref)
                    outputs = tuple(
                        item for item in description.outputs if item.name == description_output
                    )
                    if len(outputs) != 1:
                        raise NousError(
                            "media.description_mismatch", "Description must have one primary output"
                        )
                    output = outputs[0]
                    structured_inputs = scope.model_copy(
                        update={
                            "sources": (
                                *scope.sources,
                                SelectedSource(
                                    slot_key="DESCRIPTION",
                                    role="context",
                                    display_name="Description",
                                    selections=(ContentSelection(artifact=output.artifact),),
                                ),
                            )
                        }
                    )
                generated = await self.generation.compute(
                    GenerationRequest(
                        task=structure_task,
                        inputs=structured_inputs,
                        question=request.question,
                        instructions=request.instructions,
                        output_schema=output_schema,
                        input_mode=unit.mode,
                        document_mode=request.document_mode,
                        without_audio=request.without_audio,
                    ),
                    call,
                )
                saved = await self.results.adopt(facts=generated.facts, outputs=generated.outputs)
                results.append(saved.ref)
                coverage.append(saved.coverage)
                await call.saved(saved.ref)
        except (NousError, asyncio.CancelledError, TimeoutError) as error:
            add_error_context(
                error,
                {
                    "failed_stage": failed_stage,
                    "original": original.artifact.model_dump(mode="json"),
                    "completed_refs": [
                        item.model_dump(mode="json") for item in (*descriptions, *results)
                    ],
                    "unfinished_ranges": [
                        item.model_dump(mode="json") for item in ranges[len(coverage) :]
                    ],
                },
            )
            raise
        return AnalysisOutcome(
            original=original.artifact,
            descriptions=tuple(descriptions),
            results=tuple(results),
            coverage={"actual": coverage},
        )
