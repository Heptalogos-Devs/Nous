"""Three media strategies over original selections and durable stage results."""

from __future__ import annotations

import asyncio
from collections.abc import Sequence
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING
from uuid import uuid4

from pydantic import BaseModel, ConfigDict, JsonValue, TypeAdapter, model_validator

from nous.common.errors import NousError
from nous.common.operations import (
    OperationIssue,
    add_error_context,
    record_completed,
    report_outcome,
)
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    InputSpec,
    MediaStrategy,
    OutputContent,
    ProcessingResult,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceFacts,
    SourceRole,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.models.generation import FrozenLanguageTask, GenerationOperations, GenerationRequest
from nous.runtime.execution import ExecutionManager

from .contracts import AnalysisResult
from .preparation import MediaPreparer

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot


class MediaRequest(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    key: str
    source: ContentSelection
    slot_key: str
    role: SourceRole
    display_name: str | None
    facts: tuple[SourceFacts, ...]
    strategy: MediaStrategy
    question: str
    instructions: str
    description_task: str | None
    description_task_ref: ArtifactRef | None
    extraction_task_ref: ArtifactRef | None
    descriptions: tuple[str, ...]
    document_mode: str
    without_audio: bool
    output_schema: dict[str, JsonValue] | None
    description_name: str

    @model_validator(mode="after")
    def valid_stages(self) -> MediaRequest:
        if any(fact.artifact != self.source.artifact for fact in self.facts):
            raise ValueError("Media facts must belong to the selected original")
        if self.strategy == "direct_structured" and self.descriptions:
            raise ValueError("Direct analysis cannot reuse descriptions")
        if self.strategy != "description_only" and self.extraction_task_ref is None:
            raise ValueError("Structured analysis requires a frozen extraction task")
        if (
            self.strategy != "direct_structured"
            and not self.descriptions
            and self.description_task_ref is None
        ):
            raise ValueError("Description analysis requires a frozen description task")
        return self


class MediaBatch(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    inputs: SelectedInputs
    coverage: dict[str, JsonValue]


class MediaAnalysis:
    """Organize description and structured interpretation of the same original materials."""

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

    async def _reuse_description(
        self,
        reference: str,
        selected: SelectedInputs,
        task: str | None,
        question: str,
        instructions: str,
    ) -> tuple[ProcessingResult, SelectedInputs]:
        result = await self.results.get(reference)
        saved = result.parameters.get("media_analysis")
        if not isinstance(saved, dict):
            raise NousError(
                "media.description_mismatch",
                "Result is not a saved media description",
                details={"field": "media_analysis"},
            )
        expected = {
            "inputs": selected.identity(),
            "question": question,
            "instructions": instructions,
        }
        if task is not None:
            expected["task"] = task
        for field, value in expected.items():
            if saved.get(field) != value:
                raise NousError(
                    "media.description_mismatch",
                    "Description does not match this analysis",
                    details={"field": field},
                )
        if result.operation != saved.get("task"):
            raise NousError(
                "media.description_mismatch",
                "Description task does not match its saved request",
                details={"field": "task"},
            )
        scope = SelectedInputs.model_validate(saved["scope"])
        return result, scope

    async def _batch_scopes(
        self,
        task: FrozenLanguageTask,
        selected: SelectedInputs,
        snapshot: ConfigSnapshot,
        request: MediaRequest,
        key: str,
        request_ref: ResultRef,
    ) -> tuple[MediaBatch, ...]:
        result = await self.results.find_producer(key)
        if result is not None:
            output = next(item.artifact for item in result.outputs if item.name == "scopes")
            return TypeAdapter(tuple[MediaBatch, ...]).validate_json(
                await self.artifacts.read_bytes(output)
            )
        frozen = await self.generation.load_language_task(task, snapshot)
        plan = await self.generation.prepare_input(
            frozen,
            selected,
            snapshot,
            document_mode=request.document_mode,
            without_audio=request.without_audio,
            per_batch=True,
        )
        scopes = tuple(
            MediaBatch(inputs=group.route_inputs, coverage=group.coverage)
            for group in plan.input_plan.request_groups
        )
        await self.results.adopt(
            operation="media.batch_plan",
            producer={"kind": "deterministic"},
            inputs=(),
            outputs={
                "scopes": OutputContent(
                    data=TypeAdapter(tuple[MediaBatch, ...]).dump_json(scopes),
                    media_type="application/json",
                    display_name="media-scopes.json",
                )
            },
            dependencies=(request_ref, task.descriptor),
            producer_key=key,
        )
        return scopes

    async def _stage(
        self,
        task: FrozenLanguageTask,
        inputs: SelectedInputs,
        snapshot: ConfigSnapshot,
        request: MediaRequest,
        key: str,
        request_ref: ResultRef,
        *,
        dependencies: tuple[ResultRef, ...] = (),
        context: tuple[InputGroup, ...] = (),
        parameters: dict[str, JsonValue] | None = None,
        schema: dict[str, JsonValue] | None = None,
    ) -> ProcessingResult:
        result_key = key + ":media-result"
        existing = await self.results.find_producer(result_key)
        if existing is not None:
            return existing
        produced = await self.generation.produce(
            GenerationRequest(
                task=task,
                inputs=inputs,
                question=request.question,
                instructions=request.instructions,
                schema=schema,
                mode="independent_batches",
                document_mode=request.document_mode,
                without_audio=request.without_audio,
                dependencies=(request_ref, *dependencies),
                context=context,
            ),
            computation_key=key,
            snapshot=snapshot,
        )
        if parameters:
            produced = replace(produced, parameters={**produced.parameters, **parameters})
        prepared = await self.generation.prepare_adoption(produced, producer_key=result_key)
        async with self.results.database.write() as tx:
            result = await self.generation.adopt_in(tx, prepared)
        return result

    async def analyze(
        self,
        input: InputSpec | Path | Artifact | ArtifactRef | str,
        strategy: MediaStrategy,
        snapshot: ConfigSnapshot,
        *,
        selection: SourceSelection | None = None,
        question: str = "",
        instructions: str = "",
        task: str | None = None,
        description_task: str | None = None,
        document_mode: str = "auto",
        without_audio: bool = False,
        description_result: str | Sequence[str] | None = None,
        schema: dict[str, JsonValue] | None = None,
    ) -> AnalysisResult:
        """Capture one original and continue its durable media analysis."""
        async with self.execution.operation("media.analyze", snapshot):
            if isinstance(input, InputSpec):
                original = await self.artifacts.accept(input)
                selected = selection or input.selection
                role, slot, display_name = input.role, input.slot_key, input.display_name
            elif isinstance(input, Path):
                original = await self.artifacts.ingest(input)
                selected, role, slot, display_name = (
                    selection or SourceSelection(),
                    "source",
                    None,
                    None,
                )
            elif isinstance(input, Artifact):
                original = input
                selected, role, slot, display_name = (
                    selection or SourceSelection(),
                    "source",
                    None,
                    None,
                )
            else:
                original = await self.artifacts.get(input)
                selected, role, slot, display_name = (
                    selection or SourceSelection(),
                    "source",
                    None,
                    None,
                )
            selected_inputs = await self.generation.accept_inputs(
                InputSpec(
                    kind="artifact",
                    artifact=original.ref,
                    selection=selected,
                    role=role,
                    slot_key=slot,
                    display_name=display_name,
                ),
                snapshot,
            )
            if len(selected_inputs.sources) != 1 or len(selected_inputs.sources[0].selections) != 1:
                raise NousError(
                    "media.invalid_input", "Media analysis requires one original selection"
                )
            source = selected_inputs.sources[0]
            reusable = (
                [description_result]
                if isinstance(description_result, str)
                else list(description_result or ())
            )
            if reusable and strategy == "direct_structured":
                raise NousError(
                    "media.description_mismatch",
                    "Direct structured analysis does not accept a description result",
                )
            settings = snapshot.values.media
            description_name = description_task or settings.analysis.description_task
            extraction_name = task or settings.analysis.extraction_task
            extraction_ref = (
                await self.generation.freeze_language_task(
                    extraction_name, snapshot, inputs=selected_inputs, without_audio=without_audio
                )
                if strategy != "description_only"
                else None
            )
            description_ref = (
                await self.generation.freeze_language_task(
                    description_name, snapshot, inputs=selected_inputs, without_audio=without_audio
                )
                if not reusable and strategy != "direct_structured"
                else None
            )
            request = MediaRequest(
                key="media:" + str(uuid4()),
                source=source.selections[0],
                slot_key=source.slot_key,
                role=source.role,
                display_name=source.display_name,
                facts=selected_inputs.facts,
                strategy=strategy,
                question=question,
                instructions=instructions,
                description_task=description_task,
                description_task_ref=description_ref.descriptor if description_ref else None,
                extraction_task_ref=extraction_ref.descriptor if extraction_ref else None,
                descriptions=tuple(reusable),
                document_mode=document_mode,
                without_audio=without_audio,
                output_schema=schema,
                description_name=description_name,
            )
            recorded = await self.results.adopt(
                operation="media.analysis_request",
                producer={"kind": "user"},
                inputs=(
                    InputGroup(
                        key=source.slot_key,
                        slot_key=source.slot_key,
                        role=source.role,
                        name=source.display_name or source.slot_key,
                        parts=(InputPart(content=request.source),),
                    ),
                ),
                outputs={
                    "request": OutputContent(
                        data=request.model_dump_json().encode(),
                        media_type="application/json",
                        display_name="media-request.json",
                    )
                },
                dependencies=tuple(
                    ref
                    for ref in (request.description_task_ref, request.extraction_task_ref)
                    if ref
                ),
                producer_key=request.key + ":request",
            )
            return await self._continue(request, recorded.ref, snapshot)

    async def _continue(
        self, request: MediaRequest, request_ref: ResultRef, snapshot: ConfigSnapshot
    ) -> AnalysisResult:
        completed: list[str] = []
        descriptions: list[ProcessingResult] = []
        final: list[ProcessingResult] = []
        pending: list[dict[str, JsonValue]] = []
        stage = "prepare"
        original: Artifact | None = None
        try:
            original = await self.artifacts.get(request.source.artifact)
            selected = request.source.selection
            selected_inputs = SelectedInputs(
                sources=(
                    SelectedSource(
                        slot_key=request.slot_key,
                        role=request.role,
                        display_name=request.display_name,
                        selections=(request.source,),
                    ),
                ),
                facts=request.facts,
            )
            strategy, question, instructions = (
                request.strategy,
                request.question,
                request.instructions,
            )
            description_name = request.description_name
            reusable = request.descriptions
            stages: list[tuple[ProcessingResult | None, SelectedInputs]] = []
            for reference in reusable:
                description, scope = await self._reuse_description(
                    reference, selected_inputs, request.description_task, question, instructions
                )
                descriptions.append(description)
                completed.append(description.ref.identifier)
                stages.append((description, scope))
            description_ref = (
                FrozenLanguageTask(descriptor=request.description_task_ref)
                if request.description_task_ref
                else None
            )
            extraction_ref = (
                FrozenLanguageTask(descriptor=request.extraction_task_ref)
                if request.extraction_task_ref
                else None
            )
            if not reusable and strategy != "direct_structured":
                assert description_ref is not None
                assert request_ref is not None
                scopes = await self._batch_scopes(
                    description_ref,
                    selected_inputs,
                    snapshot,
                    request,
                    request.key + ":description-plan",
                    request_ref,
                )
                pending = [batch.coverage for batch in scopes]
                for index, batch in enumerate(scopes):
                    scope = batch.inputs
                    stage = "description"
                    description = await self._stage(
                        description_ref,
                        scope,
                        snapshot,
                        request,
                        f"{request.key}:description:{index}",
                        request_ref,
                        parameters=TypeAdapter(dict[str, JsonValue]).validate_python(
                            {
                                "media_analysis": {
                                    "inputs": selected_inputs.identity(),
                                    "scope": scope.model_dump(mode="json"),
                                    "task": description_name,
                                    "question": question,
                                    "instructions": instructions,
                                }
                            }
                        ),
                    )
                    descriptions.append(description)
                    completed.append(description.ref.identifier)
                    stages.append((description, scope))
                    pending.pop(0)
            elif not reusable:
                stages.append((None, selected_inputs))

            if strategy == "description_only":
                final.extend(descriptions)
            else:
                assert extraction_ref is not None and request_ref is not None
                for stage_index, (description, scope) in enumerate(stages):
                    stage = "structure"
                    scopes = await self._batch_scopes(
                        extraction_ref,
                        scope,
                        snapshot,
                        request,
                        f"{request.key}:structure-plan:{stage_index}",
                        request_ref,
                    )
                    pending = [batch.coverage for batch in scopes]
                    dependencies: tuple[ResultRef, ...] = ()
                    extra: tuple[InputGroup, ...] = ()
                    if description is not None:
                        output = next(
                            item.artifact for item in description.outputs if item.name == "output"
                        )
                        artifact = await self.artifacts.get(output)
                        extra = (
                            InputGroup(
                                key="DESCRIPTION",
                                slot_key="DESCRIPTION",
                                role="context",
                                name="Previously completed media description",
                                parts=(
                                    InputPart(
                                        content=ContentSelection(artifact=output),
                                        representation="description",
                                        media_type=artifact.media_type,
                                        source_media_type=artifact.media_type,
                                    ),
                                ),
                            ),
                        )
                        dependencies = (description.ref,)
                    for index, batch in enumerate(scopes):
                        result = await self._stage(
                            extraction_ref,
                            batch.inputs,
                            snapshot,
                            request,
                            f"{request.key}:structure:{stage_index}:{index}",
                            request_ref,
                            dependencies=dependencies,
                            context=extra,
                            schema=request.output_schema,
                        )
                        final.append(result)
                        completed.append(result.ref.identifier)
                        pending.pop(0)
            return AnalysisResult(
                original,
                selected,
                tuple(final),
                tuple(descriptions),
                {
                    "selection": selected.model_dump(mode="json"),
                    "actual": [result.coverage for result in final],
                },
                preparations=tuple(
                    ref
                    for ref in completed
                    if ref not in {item.ref.identifier for item in (*descriptions, *final)}
                ),
            )
        except (NousError, OSError, asyncio.CancelledError, TimeoutError) as exc:
            record_completed(completed)
            if isinstance(exc, NousError):
                report_outcome(
                    "partial",
                    issues=(
                        OperationIssue(
                            stage=stage,
                            code=exc.code,
                            message=exc.message,
                            details={
                                **exc.details,
                                "unfinished_coverage": list[JsonValue](pending),
                            },
                            retryable=exc.retryable,
                            affected_refs=(original.ref.identifier,) if original else (),
                            completed_refs=tuple(dict.fromkeys(completed)),
                        ),
                    ),
                )
            add_error_context(
                exc,
                {
                    "completed_result_refs": list(dict.fromkeys(completed)),
                    "description_result_refs": [item.ref.identifier for item in descriptions],
                    "failed_stage": stage,
                    "stage": stage,
                    "unfinished_coverage": pending,
                    "artifact": original.ref.identifier if original else None,
                    "media_request_ref": request_ref.model_dump(mode="json"),
                },
            )
            raise

    async def recover(self, ref: str | ResultRef, snapshot: ConfigSnapshot) -> AnalysisResult:
        """Continue an accepted media analysis without repeating completed stages."""
        async with self.execution.operation("media.recover", snapshot):
            result = await self.results.get(ref)
            if result.operation != "media.analysis_request":
                raise NousError(
                    "media.request_required", "Recovery requires a media analysis request"
                )
            output = next(item.artifact for item in result.outputs if item.name == "request")
            request = MediaRequest.model_validate_json(await self.artifacts.read_bytes(output))
            return await self._continue(request, result.ref, snapshot)
