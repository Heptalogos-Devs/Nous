"""Three explicit analysis strategies; each completed language stage is durable."""

from __future__ import annotations

import asyncio
from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.common.operations import add_error_context, record_completed
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
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.models.operations import GenerationOperations, PendingGeneration
from nous.runtime.execution import ExecutionManager

from .contracts import AnalysisResult, PreparedBatch, PreparedSource
from .preparation import MediaPreparer

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot


class MediaAnalysis:
    """Run explicit durable interpretation stages over shared semantic input groups."""

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
                or source.role != actual.role
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
            async with self.execution.operation("media.analyze", snapshot):
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
                first_task = frozen_description or frozen_extraction
                assert first_task is not None
                prepared = await self.generation.prepare_input(
                    first_task,
                    original,
                    snapshot,
                    selection=selection,
                    document_mode=document_mode,
                    without_audio=without_audio,
                    per_batch=True,
                )
                completed.extend(prepared.preparations)
                pending = [group.coverage for group in prepared.batches]
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
                for ordinal, group in enumerate(prepared.batches):
                    description: ProcessingResult | None = None
                    if reusable:
                        request = await self.generation.prepare_group(
                            prepared.frozen,
                            group,
                            snapshot,
                            question=question,
                            instructions=instructions,
                            preview=True,
                            completed=completed,
                        )
                        reading = (
                            request.reading
                            if isinstance(request, PendingGeneration)
                            else request.compiled.reading
                        )
                        expected = PreparedBatch(
                            reading.sources,
                            tuple(source.group for source in reading.sources if source.group),
                            SourceSelection(),
                            request.coverage,
                        )
                        description = await self._reuse_description(
                            reusable[ordinal], expected, description_task
                        )
                        record_completed(description.ref.address)
                    if frozen_description is not None:
                        stage = "description"
                        description = description or await self.generation.run_group(
                            prepared.frozen,
                            group,
                            snapshot,
                            question=question,
                            instructions=instructions,
                            completed=completed,
                        )
                        descriptions.append(description)
                        completed.append(description.ref.address)
                        if strategy == "description_only":
                            final.append(description)
                    if frozen_extraction is not None:
                        stage = "structure"
                        extra_sources: list[PreparedSource] = []
                        if description is not None:
                            output = next(
                                output for output in description.outputs if output.name == "output"
                            )
                            artifact = await self.artifacts.get(output.artifact)
                            extra_sources.append(
                                PreparedSource(
                                    "DESCRIPTION",
                                    "Previously completed media description",
                                    text=await self.artifacts.read_text(output.artifact),
                                    group=InputGroup(
                                        key="DESCRIPTION",
                                        name="Previously completed media description",
                                        parts=(
                                            InputPart(
                                                content=ContentSelection(artifact=output.artifact),
                                                representation="description",
                                                media_type=artifact.media_type,
                                                source_media_type=artifact.media_type,
                                            ),
                                        ),
                                    ),
                                    metadata={"representation": "description"},
                                )
                            )
                        stage_task = (
                            prepared.frozen
                            if strategy == "direct_structured"
                            else frozen_extraction
                        )
                        result = await self.generation.run_group(
                            stage_task,
                            group,
                            snapshot,
                            question=question,
                            instructions=instructions,
                            schema=schema,
                            extra_sources=extra_sources,
                            completed=completed,
                        )
                        final.append(result)
                        completed.append(result.ref.address)
                    pending.pop(0)
                return AnalysisResult(
                    original,
                    selection or SourceSelection(),
                    tuple(final),
                    tuple(descriptions),
                    {**prepared.coverage, "actual": [result.coverage for result in final]},
                    preparations=tuple(
                        ref
                        for ref in completed
                        if ref not in {result.ref.address for result in (*descriptions, *final)}
                    ),
                )
        except (NousError, asyncio.CancelledError, TimeoutError) as exc:
            record_completed(completed)
            add_error_context(
                exc,
                {
                    "completed_result_refs": list(dict.fromkeys(completed)),
                    "failed_stage": stage,
                    "unfinished_coverage": pending,
                    "artifact": original.ref.address if original else None,
                },
            )
            raise
