"""Three explicit analysis strategies; each completed language stage is durable."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Sequence
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    InputSpec,
    MediaStrategy,
    ProcessingResult,
    SourceSelection,
)
from nous.content.store import ArtifactStore, ResultStore
from nous.models.contracts import ReadingInput
from nous.models.operations import GenerationOperations
from nous.runtime.execution import ExecutionManager

from .contracts import AnalysisResult, PreparedBatch, PreparedSource
from .preparation import MediaPreparer

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot


class MediaAnalysis:
    """Run interpretation strategies as durable stages over one prepared representation."""

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

    @staticmethod
    def _requirements(task: str, snapshot: ConfigSnapshot) -> tuple[str, ...]:
        config = snapshot.typed_values()
        assignment = config.tasks.get(task)
        if assignment is None:
            raise NousError(
                "model.task_missing", "Media task is not configured", details={"task": task}
            )
        return tuple(config.models[assignment.model].input_formats)

    async def _reuse_description(
        self,
        reference: str,
        batch: PreparedBatch,
        description_task: str,
    ) -> ProcessingResult:
        """Reuse a description only for matching task, exact inputs and measured coverage."""
        description = await self.results.get(reference)
        if description.operation != description_task:
            raise NousError(
                "media.description_mismatch", "Result is not a matching media description"
            )
        expected = {group.key: group for group in batch.inputs}
        matching = len(expected) == len(description.inputs)
        for actual in description.inputs:
            source = expected.get(actual.key)
            if (
                source is None
                or source.name != actual.name
                or len(source.parts) != len(actual.parts)
            ):
                matching = False
                break
            for wanted, used in zip(source.parts, actual.parts, strict=True):
                same = wanted == used
                transcript = (
                    wanted.representation == "audio"
                    and used.representation == "transcript"
                    and any(
                        origin.origin == wanted.content and origin.transform_ref is not None
                        for origin in used.origins
                    )
                )
                if not (same or transcript):
                    matching = False
        if not matching or description.coverage != batch.coverage:
            raise NousError(
                "media.description_mismatch",
                "Description sources, selection or coverage differ from this batch",
            )
        return description

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
        """Commit successful stages as they finish and report unfinished batch coverage on failure."""
        completed: list[str] = []
        descriptions: list[ProcessingResult] = []
        final: list[ProcessingResult] = []
        stage = "prepare"
        pending: list[dict[str, JsonValue]] = []
        original: Artifact | None = None
        try:
            async with self.execution.operation("media.analyze", snapshot) as context:
                if isinstance(input, InputSpec):
                    original = await self.artifacts.accept(input)
                    selection = selection or input.selection
                elif isinstance(input, Path):
                    original = await self.artifacts.ingest(input)
                elif isinstance(input, Artifact):
                    original = input
                else:
                    original = await self.artifacts.get(input)
                settings = snapshot.typed_values().media
                if description_result and strategy == "direct_structured":
                    raise NousError(
                        "media.description_mismatch",
                        "Direct structured analysis does not accept a description result",
                    )
                description_task = description_task or settings.analysis.description_task
                extraction_task = task or settings.analysis.extraction_task
                language_task = (
                    description_task if strategy == "description_only" else extraction_task
                )
                requirements = self._requirements(language_task, snapshot)
                frozen_description = (
                    self.generation.executor.freeze(description_task, snapshot)
                    if strategy != "direct_structured"
                    else None
                )
                frozen_extraction = (
                    self.generation.executor.freeze(extraction_task, snapshot)
                    if strategy != "description_only"
                    else None
                )
                frozen_transcription = self.generation.freeze_transcription(snapshot)
                if strategy == "describe_then_structure":
                    requirements = self._requirements(description_task, snapshot)
                prepared = await self.preparer.prepare(
                    original,
                    selection,
                    requirements=requirements,
                    settings=settings,
                    document_mode=document_mode,
                    without_audio=without_audio,
                )
                completed.extend(prepared.completed_result_refs)
                pending = [batch.coverage for batch in prepared.batches]
                reusable = (
                    [description_result]
                    if isinstance(description_result, str)
                    else list(description_result or ())
                )
                if reusable and len(reusable) != len(prepared.batches):
                    raise NousError(
                        "media.description_mismatch",
                        "Provide one description result for each prepared batch",
                    )
                for ordinal, plan in enumerate(prepared.batches):
                    batch = await self.preparer.materialize(plan)
                    description: ProcessingResult | None = None
                    if reusable:
                        description = await self._reuse_description(
                            reusable[ordinal], batch, description_task
                        )
                        batch = replace(
                            batch,
                            inputs=description.inputs,
                            sources=tuple(
                                PreparedSource(group.key, group.name, group=group)
                                for group in description.inputs
                            ),
                        )
                        batch = await self.preparer.materialize(batch)
                    reading = ReadingInput(question, instructions, batch.sources)
                    first_task = frozen_description or frozen_extraction
                    assert first_task is not None
                    stage = "transcription"
                    reading = await self.generation.language_sources(
                        reading, first_task, snapshot, context, completed, frozen_transcription
                    )
                    batch = replace(
                        batch,
                        sources=reading.sources,
                        inputs=tuple(
                            source.group for source in reading.sources if source.group is not None
                        ),
                    )
                    if strategy in {"description_only", "describe_then_structure"}:
                        stage = "description"
                        description = description or await self.generation.run(
                            frozen_description or description_task,
                            reading,
                            snapshot,
                            inputs=batch.inputs,
                            coverage=batch.coverage,
                            context=context,
                            transcription=frozen_transcription,
                        )
                        descriptions.append(description)
                        completed.extend(
                            str(ref)
                            for ref in cast(
                                list[JsonValue], description.parameters.get("stage_result_refs", [])
                            )
                            if isinstance(ref, str)
                        )
                        completed.append(description.ref.address)
                        if strategy == "description_only":
                            final.append(description)
                    if strategy in {"direct_structured", "describe_then_structure"}:
                        stage = "structure"
                        # Keep the actual media inputs alongside the persisted description.
                        sources = list(batch.sources)
                        inputs = list(batch.inputs)
                        if description is not None:
                            output = next(
                                output for output in description.outputs if output.name == "output"
                            )
                            text = await self.artifacts.read_text(output.artifact)
                            sources.append(
                                PreparedSource(
                                    "DESCRIPTION",
                                    "Previously completed media description",
                                    text=text,
                                    group=InputGroup(
                                        key="DESCRIPTION",
                                        name="Previously completed media description",
                                        parts=(
                                            InputPart(
                                                content=ContentSelection(artifact=output.artifact),
                                                representation="description",
                                            ),
                                        ),
                                    ),
                                    metadata={"representation": "description"},
                                )
                            )
                            inputs.append(
                                InputGroup(
                                    key="DESCRIPTION",
                                    name="Previously completed media description",
                                    parts=(
                                        InputPart(
                                            content=ContentSelection(artifact=output.artifact),
                                            representation="description",
                                        ),
                                    ),
                                )
                            )
                        result = await self.generation.run(
                            frozen_extraction or extraction_task,
                            ReadingInput(question, instructions, tuple(sources)),
                            snapshot,
                            inputs=inputs,
                            coverage=batch.coverage,
                            context=context,
                            schema=schema,
                            transcription=frozen_transcription,
                        )
                        final.append(result)
                        completed.extend(
                            str(ref)
                            for ref in cast(
                                list[JsonValue], result.parameters.get("stage_result_refs", [])
                            )
                            if isinstance(ref, str)
                        )
                        completed.append(result.ref.address)
                    pending.pop(0)
                return AnalysisResult(
                    original,
                    prepared.selection,
                    tuple(final),
                    tuple(descriptions),
                    prepared.coverage,
                    preparations=tuple(
                        ref
                        for ref in completed
                        if ref not in {result.ref.address for result in (*descriptions, *final)}
                    ),
                )
        except (asyncio.CancelledError, TimeoutError) as exc:
            exc.add_note(
                json.dumps(
                    {
                        "completed_result_refs": completed,
                        "failed_stage": stage,
                        "unfinished_coverage": pending,
                        "artifact": original.ref.address if original else None,
                    }
                )
            )
            raise
        except NousError as exc:
            details: dict[str, object] = dict(exc.details)
            previous = details.get("completed_result_refs", [])
            previous_refs = (
                [value for value in cast(list[object], previous) if isinstance(value, str)]
                if isinstance(previous, list)
                else []
            )
            details.update(
                {
                    "completed_result_refs": list(dict.fromkeys([*previous_refs, *completed])),
                    "failed_stage": stage,
                    "unfinished_coverage": pending,
                    "artifact": original.ref.address if original is not None else None,
                }
            )
            raise NousError(
                exc.code,
                exc.message,
                operation="media.analyze",
                details=details,
                cause=exc,
                retryable=False,
            ) from exc
