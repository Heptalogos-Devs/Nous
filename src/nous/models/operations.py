"""One generation use case shared by model tools and media analysis."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import TYPE_CHECKING

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    OutputContent,
    ProcessingResult,
    ResultInput,
    SourceSelection,
)
from nous.content.store import ResultStore
from nous.runtime.execution import ExecutionManager, OperationContext

from .contracts import GenerationResult, PreparedSource, ReadingInput

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot
    from nous.media.preparation import MediaPreparer

    from .executor import ModelExecutor, PreparedGeneration


@dataclass(frozen=True)
class PreparedGenerationBatch:
    prepared: PreparedGeneration
    inputs: tuple[ResultInput, ...]
    coverage: dict[str, JsonValue]


@dataclass(frozen=True)
class PreparedRun:
    artifacts: tuple[Artifact, ...]
    batches: tuple[PreparedGenerationBatch, ...]
    coverage: dict[str, JsonValue]


@dataclass(frozen=True)
class GenerationOutcome:
    artifacts: tuple[Artifact, ...]
    results: tuple[ProcessingResult, ...]
    coverage: dict[str, JsonValue]


class GenerationOperations:
    def __init__(
        self,
        executor: ModelExecutor,
        results: ResultStore,
        execution: ExecutionManager,
        preparer: MediaPreparer | None = None,
    ) -> None:
        self.executor, self.results, self.execution = executor, results, execution
        self.preparer = preparer

    async def prepare_input(
        self,
        task: str,
        input: Path | Artifact | ArtifactRef | str | Sequence[Path | Artifact | ArtifactRef | str],
        snapshot: ConfigSnapshot,
        *,
        selection: SourceSelection | None = None,
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        document_mode: str = "auto",
        without_audio: bool = False,
        texts: Sequence[str] = (),
    ) -> PreparedRun:
        async with self.execution.operation("model.prepare_input", snapshot) as context:
            if self.preparer is None:
                raise NousError("model.preparation_unavailable", "Media preparer is not attached")
            originals: list[Artifact] = []
            inputs = [input] if isinstance(input, (Path, Artifact, ArtifactRef, str)) else input
            for source in inputs:
                if isinstance(source, Path):
                    originals.append(await self.results.artifacts.ingest(source))
                elif isinstance(source, Artifact):
                    originals.append(source)
                else:
                    originals.append(await self.results.artifacts.get(source))
            for ordinal, text in enumerate(texts, 1):
                originals.append(
                    await self.results.artifacts.ingest(text, display_name=f"text-{ordinal}.txt")
                )
            if not originals:
                raise NousError("model.input_required", "Provide an input artifact, file or text")
            config = snapshot.typed_values()
            assignment, routes = self.executor.registry.resolve(task, snapshot)
            if assignment.operation != "generation":
                raise NousError(
                    "model.operation_mismatch", "Use the transcription operation for this task"
                )
            requirements = tuple(
                mime
                for mime in routes[0].model.input_formats
                if all(mime in route.model.input_formats for route in routes)
            )
            batches: list[PreparedGenerationBatch] = []
            pending_sources: list[PreparedSource] = []
            pending_inputs: list[ResultInput] = []
            pending_coverage: list[JsonValue] = []
            byte_length = 0
            summaries: list[JsonValue] = []

            def flush() -> None:
                if pending_sources:
                    batches.append(
                        PreparedGenerationBatch(
                            self.executor.prepare(
                                task,
                                ReadingInput(question, instructions, tuple(pending_sources)),
                                snapshot,
                                schema=schema,
                            ),
                            tuple(pending_inputs),
                            {"inputs": list(pending_coverage)},
                        )
                    )
                    pending_sources.clear()
                    pending_inputs.clear()
                    pending_coverage.clear()

            for index, artifact in enumerate(originals, 1):
                prepared = await self.preparer.prepare(
                    artifact,
                    selection,
                    requirements=requirements,
                    settings=config.media,
                    snapshot=snapshot,
                    context=context,
                    document_mode=document_mode,
                    without_audio=without_audio,
                )
                summaries.append(
                    {"artifact": artifact.ref.model_dump(mode="json"), **prepared.coverage}
                )
                for batch in prepared.batches:
                    sources = [
                        replace(source, key=f"S{index}" + source.key[2:])
                        for source in batch.sources
                    ]
                    source_inputs = [
                        source.model_copy(update={"key": f"S{index}" + source.key[2:]})
                        for source in batch.inputs
                    ]
                    size = sum(
                        len((source.text or "").encode("utf-8"))
                        + sum(len(part.data) for part in source.media)
                        for source in sources
                    )
                    # Preparation owners enforce per-document/page/frame limits. Preserve their
                    # batches, combining distinct single-batch inputs when byte allowance permits.
                    if pending_sources and (
                        byte_length + size > config.media.max_inline_bytes
                        or len(prepared.batches) > 1
                    ):
                        flush()
                        byte_length = 0
                    pending_sources.extend(sources)
                    pending_inputs.extend(source_inputs)
                    pending_coverage.append(
                        {"artifact": artifact.ref.model_dump(mode="json"), **batch.coverage}
                    )
                    byte_length += size
                    if len(prepared.batches) > 1:
                        flush()
                        byte_length = 0
            flush()
            return PreparedRun(
                tuple(originals), tuple(batches), {"inputs": summaries, "batches": len(batches)}
            )

    async def run_input(
        self,
        task: str,
        input: Path | Artifact | ArtifactRef | str | Sequence[Path | Artifact | ArtifactRef | str],
        snapshot: ConfigSnapshot,
        *,
        selection: SourceSelection | None = None,
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        document_mode: str = "auto",
        without_audio: bool = False,
        on_delta: Callable[[str], None] | None = None,
        on_prepared: Callable[[PreparedRun], None] | None = None,
        on_completed: Callable[[ProcessingResult], None] | None = None,
        texts: Sequence[str] = (),
    ) -> GenerationOutcome:
        results: list[ProcessingResult] = []
        async with self.execution.operation("model.run", snapshot) as context:
            prepared = await self.prepare_input(
                task,
                input,
                snapshot,
                selection=selection,
                question=question,
                instructions=instructions,
                schema=schema,
                document_mode=document_mode,
                without_audio=without_audio,
                texts=texts,
            )
            if on_prepared is not None:
                on_prepared(prepared)
            try:
                for batch in prepared.batches:
                    result = await self.run_prepared(
                        batch.prepared,
                        inputs=batch.inputs,
                        coverage=batch.coverage,
                        context=context,
                        on_delta=on_delta,
                    )
                    results.append(result)
                    if on_completed is not None:
                        on_completed(result)
            except NousError as exc:
                exc.details.update(
                    {
                        "artifacts": [artifact.ref.address for artifact in prepared.artifacts],
                        "completed_result_refs": [result.ref.address for result in results],
                        "unfinished_coverage": [
                            batch.coverage for batch in prepared.batches[len(results) :]
                        ],
                    }
                )
                raise
            except (asyncio.CancelledError, TimeoutError) as exc:
                exc.add_note(
                    json.dumps(
                        {
                            "artifacts": [artifact.ref.address for artifact in prepared.artifacts],
                            "completed_result_refs": [result.ref.address for result in results],
                            "unfinished_coverage": [
                                batch.coverage for batch in prepared.batches[len(results) :]
                            ],
                        }
                    )
                )
                raise
            return GenerationOutcome(prepared.artifacts, tuple(results), prepared.coverage)

    async def run(
        self,
        task: str,
        reading: ReadingInput,
        snapshot: ConfigSnapshot,
        *,
        inputs: Sequence[ResultInput] | None = None,
        coverage: Mapping[str, object] | None = None,
        context: OperationContext | None = None,
        schema: dict[str, JsonValue] | None = None,
    ) -> ProcessingResult:
        return await self.run_prepared(
            self.executor.prepare(task, reading, snapshot, schema=schema),
            inputs=inputs,
            coverage=coverage,
            context=context,
        )

    async def run_prepared(
        self,
        prepared: PreparedGeneration,
        *,
        inputs: Sequence[ResultInput] | None = None,
        coverage: Mapping[str, object] | None = None,
        context: OperationContext | None = None,
        on_delta: Callable[[str], None] | None = None,
    ) -> ProcessingResult:
        async with self.execution.operation("model.generate", prepared.snapshot) as active:
            if on_delta is None:
                generated = await self.executor.execute(prepared, context=context or active)
            else:
                async with self.executor.stream(prepared, context=context or active) as stream:
                    async for delta in stream:
                        on_delta(delta)
                    if stream.result is None:
                        raise NousError(
                            "model.incomplete_stream", "The stream returned no completed result"
                        )
                    generated = stream.result
            # Adoption is outside model attempts; persistence failure never resubmits generation.
            return await self._adopt(prepared, generated, inputs, coverage)

    async def _adopt(
        self,
        prepared: PreparedGeneration,
        generated: GenerationResult,
        inputs: Sequence[ResultInput] | None,
        coverage: Mapping[str, object] | None,
    ) -> ProcessingResult:
        reading = prepared.compiled.reading
        content = generated.output
        if prepared.compiled.schema is None:
            if not isinstance(content, str):
                raise NousError("model.invalid_output", "Text generation returned a non-text value")
            output = OutputContent(data=content.encode("utf-8"))
        else:
            output = OutputContent(
                data=json.dumps(content, ensure_ascii=False, indent=2).encode("utf-8"),
                media_type="application/json",
                display_name="result.json",
            )
        outputs = {"output": output}
        if prepared.compiled.resource.manifest.contract_id == "passage-selection":
            if not isinstance(content, dict) or not isinstance(content.get("keys"), list):
                raise NousError("model.invalid_output", "Passage selection did not return keys")
            source_map = {source.key: source for source in reading.sources}
            excerpts: list[dict[str, JsonValue]] = []
            keys = content["keys"]
            if not isinstance(keys, list):
                raise NousError("model.invalid_output", "Passage selection keys are invalid")
            for key in keys:
                if not isinstance(key, str) or key not in source_map:
                    raise NousError(
                        "model.invalid_source",
                        "Selected passage key is unknown",
                        details={"key": key},
                    )
                source = source_map[key]
                if source.text is None or source.artifact is None or source.selection is None:
                    raise NousError(
                        "model.invalid_source", "Selected source has no exact text range"
                    )
                original = await self.results.artifacts.read_text(source.artifact)
                lines = original.splitlines(keepends=True)
                selection = source.selection
                if selection.start_line is None or selection.end_line is None:
                    raise NousError(
                        "model.invalid_source", "Selected source has no original line coordinates"
                    )
                exact_text = "".join(lines[selection.start_line - 1 : selection.end_line])
                excerpts.append(
                    {
                        "key": key,
                        "text": exact_text,
                        "artifact": source.artifact.model_dump(mode="json"),
                        "selection": selection.model_dump(mode="json"),
                    }
                )
            outputs["passages"] = OutputContent(
                data=json.dumps(excerpts, ensure_ascii=False, indent=2).encode("utf-8"),
                media_type="application/json",
                display_name="passages.json",
            )
        if generated.response:
            outputs["response"] = OutputContent(
                data=json.dumps(generated.response, ensure_ascii=False, indent=2).encode("utf-8"),
                media_type="application/json",
                display_name="response.json",
            )
        actual_inputs = (
            tuple(inputs)
            if inputs is not None
            else tuple(
                ResultInput(
                    artifact=source.artifact,
                    key=source.key,
                    selection=source.selection or SourceSelection(),
                    representation=str(source.metadata.get("representation", "original")),
                )
                for source in reading.sources
                if source.artifact is not None
            )
        )
        return await self.results.adopt(
            operation=prepared.task,
            producer=generated.producer,
            inputs=actual_inputs,
            outputs=outputs,
            request=generated.request,
            parameters={
                "attempts": generated.attempts,
                "usage": generated.usage,
                "finish_reason": generated.finish_reason,
            },
            coverage=coverage,
        )
