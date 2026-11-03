"""Three explicit analysis strategies; each completed language stage is durable."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    MediaStrategy,
    ProcessingResult,
    ResultInput,
    SourceSelection,
)
from nous.content.store import ArtifactStore, ResultStore
from nous.models.contracts import PreparedSource, ReadingInput
from nous.models.operations import GenerationOperations
from nous.runtime.execution import ExecutionManager

from .contracts import AnalysisResult, PreparedBatch
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
        if description.inputs != batch.inputs or description.coverage != batch.coverage:
            raise NousError(
                "media.description_mismatch",
                "Description sources, selection or coverage differ from this batch",
            )
        return description

    async def analyze(
        self,
        input: Path | Artifact | ArtifactRef | str,
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
                if isinstance(input, Path):
                    original = await self.artifacts.ingest(input)
                elif isinstance(input, Artifact):
                    original = input
                else:
                    original = await self.artifacts.get(input)
                settings = snapshot.typed_values().media
                description_task = description_task or settings.analysis.description_task
                extraction_task = task or settings.analysis.extraction_task
                language_task = (
                    description_task if strategy == "description_only" else extraction_task
                )
                requirements = self._requirements(language_task, snapshot)
                if strategy == "describe_then_structure":
                    first_requirements = self._requirements(description_task, snapshot)
                    requirements = tuple(
                        mime for mime in requirements if mime in first_requirements
                    )
                prepared = await self.preparer.prepare(
                    original,
                    selection,
                    requirements=requirements,
                    settings=settings,
                    snapshot=snapshot,
                    context=context,
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
                for ordinal, batch in enumerate(prepared.batches):
                    reading = ReadingInput(question, instructions, batch.sources)
                    description: ProcessingResult | None = None
                    if strategy in {"description_only", "describe_then_structure"}:
                        stage = "description"
                        description = (
                            await self._reuse_description(
                                reusable[ordinal], batch, description_task
                            )
                            if reusable
                            else await self.generation.run(
                                description_task,
                                reading,
                                snapshot,
                                inputs=batch.inputs,
                                coverage=batch.coverage,
                                context=context,
                            )
                        )
                        descriptions.append(description)
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
                                    artifact=output.artifact,
                                    metadata={"representation": "description"},
                                )
                            )
                            inputs.append(
                                ResultInput(
                                    artifact=output.artifact,
                                    key="DESCRIPTION",
                                    representation="description",
                                )
                            )
                        result = await self.generation.run(
                            extraction_task,
                            ReadingInput(question, instructions, tuple(sources)),
                            snapshot,
                            inputs=inputs,
                            coverage=batch.coverage,
                            context=context,
                            schema=schema,
                        )
                        final.append(result)
                        completed.append(result.ref.address)
                    pending.pop(0)
                return AnalysisResult(
                    original,
                    prepared.selection,
                    tuple(final),
                    tuple(descriptions),
                    prepared.coverage,
                    preparations=prepared.completed_result_refs,
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
