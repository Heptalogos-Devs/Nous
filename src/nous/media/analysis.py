"""Three media strategies over original selections and durable stage results."""

from __future__ import annotations

import asyncio
from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING

from pydantic import JsonValue, TypeAdapter

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
    ProcessingResult,
    ResultRef,
    SelectedInputs,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.models.generation import GenerationOperations
from nous.runtime.execution import ExecutionManager

from .contracts import AnalysisResult, PreparedSource
from .preparation import MediaPreparer

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot


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
        """Adopt each successful stage and resume from formal description references."""
        completed: list[str] = []
        descriptions: list[ProcessingResult] = []
        final: list[ProcessingResult] = []
        pending: list[dict[str, JsonValue]] = []
        stage = "prepare"
        original: Artifact | None = None
        async with self.execution.operation("media.analyze", snapshot):
            try:
                if isinstance(input, InputSpec):
                    original = await self.artifacts.accept(input)
                    selected = selection or input.selection
                    role, slot = input.role, input.slot_key
                elif isinstance(input, Path):
                    original = await self.artifacts.ingest(input)
                    selected, role, slot = selection or SourceSelection(), "source", None
                elif isinstance(input, Artifact):
                    original = input
                    selected, role, slot = selection or SourceSelection(), "source", None
                else:
                    original = await self.artifacts.get(input)
                    selected, role, slot = selection or SourceSelection(), "source", None
                selected_inputs = await self.generation.accept_inputs(
                    InputSpec(
                        kind="artifact",
                        artifact=original.ref,
                        selection=selected,
                        role=role,
                        slot_key=slot,
                    ),
                    snapshot,
                )
                selected = selected_inputs.sources[0].selections[0].selection
                if description_result and strategy == "direct_structured":
                    raise NousError(
                        "media.description_mismatch",
                        "Direct structured analysis does not accept a description result",
                    )
                reusable = (
                    [description_result]
                    if isinstance(description_result, str)
                    else list(description_result or ())
                )
                settings = snapshot.values.media
                description_name = description_task or settings.analysis.description_task
                extraction_name = task or settings.analysis.extraction_task
                stages: list[tuple[ProcessingResult | None, SelectedInputs]] = []

                if reusable:
                    for reference in reusable:
                        description, scope = await self._reuse_description(
                            reference, selected_inputs, description_task, question, instructions
                        )
                        descriptions.append(description)
                        completed.append(description.ref.identifier)
                        stages.append((description, scope))
                extraction_ref = None
                extraction_policy = None
                if strategy != "description_only":
                    extraction_ref = await self.generation.freeze_language_task(
                        extraction_name,
                        snapshot,
                        inputs=selected_inputs,
                        without_audio=without_audio,
                    )
                    extraction_policy = await self.generation.load_language_task(
                        extraction_ref, snapshot
                    )
                if not reusable and strategy != "direct_structured":
                    description_ref = await self.generation.freeze_language_task(
                        description_name,
                        snapshot,
                        inputs=selected_inputs,
                        without_audio=without_audio,
                    )
                    description_policy = await self.generation.load_language_task(
                        description_ref, snapshot
                    )
                    plan = await self.generation.prepare_input(
                        description_policy,
                        selected_inputs,
                        snapshot,
                        document_mode=document_mode,
                        without_audio=without_audio,
                        per_batch=True,
                    )
                    completed.extend(plan.preparations)
                    pending = [group.coverage for group in plan.batches]
                    for group in plan.batches:
                        stage = "description"
                        description = await self.generation.run_group(
                            plan.frozen,
                            group,
                            snapshot,
                            question=question,
                            instructions=instructions,
                            completed=completed,
                            frozen_task_ref=description_ref.descriptor,
                            dependencies=(description_ref.descriptor,),
                            parameters=TypeAdapter(dict[str, JsonValue]).validate_python(
                                {
                                    "media_analysis": {
                                        "inputs": selected_inputs.identity(),
                                        "scope": group.route_inputs.model_dump(mode="json"),
                                        "task": description_name,
                                        "question": question,
                                        "instructions": instructions,
                                    }
                                }
                            ),
                        )
                        descriptions.append(description)
                        completed.append(description.ref.identifier)
                        stages.append((description, group.route_inputs))
                        pending.pop(0)
                elif not reusable:
                    stages.append((None, selected_inputs))

                if strategy == "description_only":
                    final.extend(descriptions)
                else:
                    assert extraction_policy is not None and extraction_ref is not None
                    for description, scope in stages:
                        stage = "structure"
                        plan = await self.generation.prepare_input(
                            extraction_policy,
                            scope,
                            snapshot,
                            document_mode=document_mode,
                            without_audio=without_audio,
                            per_batch=True,
                        )
                        completed.extend(plan.preparations)
                        pending = [group.coverage for group in plan.batches]
                        extra: list[PreparedSource] = []
                        dependencies: tuple[ArtifactRef | ResultRef, ...] = (
                            extraction_ref.descriptor,
                        )
                        if description is not None:
                            output = next(
                                item.artifact
                                for item in description.outputs
                                if item.name == "output"
                            )
                            artifact = await self.artifacts.get(output)
                            extra.append(
                                PreparedSource(
                                    "DESCRIPTION",
                                    "Previously completed media description",
                                    text=await self.artifacts.read_text(output),
                                    group=InputGroup(
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
                                    metadata={"representation": "description"},
                                )
                            )
                            dependencies = (extraction_ref.descriptor, description.ref)
                        for group in plan.batches:
                            result = await self.generation.run_group(
                                plan.frozen,
                                group,
                                snapshot,
                                question=question,
                                instructions=instructions,
                                schema=schema,
                                extra_sources=extra,
                                completed=completed,
                                dependencies=dependencies,
                                frozen_task_ref=extraction_ref.descriptor,
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
            except (NousError, asyncio.CancelledError, TimeoutError) as exc:
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
                    },
                )
                raise
