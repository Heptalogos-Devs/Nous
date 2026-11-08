"""Complete model, media and content operations shared by local clients and the host."""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Sequence
from pathlib import Path
from typing import Literal

from pydantic import JsonValue

from nous.content.contracts import (
    InputSpec,
    ProcessingResult,
    SourceSelection,
    TextInput,
    is_text_media_type,
)
from nous.media.contracts import AnalysisResult
from nous.models.requests import PreparedGeneration
from nous.runtime.bootstrap import Runtime


async def model_discover(runtime: Runtime, provider: str) -> dict[str, object]:
    async with runtime.execution.operation("model.discover") as context:
        return {
            "provider": provider,
            "models": await runtime.models.discover(provider, context.settings),
        }


async def model_probe(runtime: Runtime, model: str, input: Path | None) -> dict[str, object]:
    outcome = await runtime.generation.probe(model, input, runtime.execution.snapshot)
    return {
        "model": model,
        "checked": "supplied_input" if input else "text",
        "results": [
            await runtime.results.read_view(result.ref.identifier) for result in outcome.results
        ],
        "coverage": outcome.coverage,
    }


async def model_run(
    runtime: Runtime,
    task: str,
    inputs: Sequence[InputSpec],
    texts: Sequence[str],
    question: str,
    instructions: str,
    schema: dict[str, JsonValue] | None,
    document_mode: str,
    without_audio: bool,
    per_batch: bool,
    details: bool,
    stream: bool,
    show_request: bool,
    on_event: Callable[[str, object], None] | None = None,
) -> dict[str, object]:
    def emit(name: str, value: object) -> None:
        if on_event is not None:
            on_event(name, value)

    def prepared(value: PreparedGeneration) -> None:
        emit("prepared", value.to_dict())

    def delta(value: str) -> None:
        emit("delta", value)

    def completed(value: ProcessingResult) -> None:
        emit("completed", value.ref.identifier)

    outcome = await runtime.generation.run_input(
        task,
        inputs,
        runtime.execution.snapshot,
        question=question,
        instructions=instructions,
        schema=schema,
        document_mode=document_mode,
        without_audio=without_audio,
        per_batch=per_batch,
        texts=texts,
        on_prepared=prepared if show_request else None,
        on_delta=delta if stream else None,
        on_completed=completed if stream else None,
    )
    return {
        "artifacts": [item.ref.identifier for item in outcome.artifacts],
        "results": [
            await runtime.results.read_view(item.ref.identifier, sources=True, details=details)
            for item in outcome.results
        ],
        "coverage": outcome.coverage,
    }


async def model_recover(runtime: Runtime, producer_key: str, details: bool) -> dict[str, object]:
    async with runtime.execution.operation("model.recover_output") as context:
        result = await runtime.generation.recover_result(producer_key, context.settings)
        return {
            "results": [
                await runtime.results.read_view(
                    result.ref.identifier, sources=True, details=details
                )
            ],
            "coverage": result.coverage,
        }


async def prompt_preview(
    runtime: Runtime,
    prompt: str,
    inputs: Sequence[InputSpec],
    texts: Sequence[str],
    question: str,
    instructions: str,
    schema: dict[str, JsonValue] | None,
    task: str | None,
    per_batch: bool,
) -> dict[str, object]:
    return dict(
        await runtime.generation.preview(
            prompt,
            inputs,
            runtime.execution.snapshot,
            texts=texts,
            question=question,
            instructions=instructions,
            schema=schema,
            task=task,
            per_batch=per_batch,
        )
    )


async def content_add(runtime: Runtime, input: Path) -> dict[str, object]:
    async with runtime.execution.operation("content.add"):
        return (await runtime.artifacts.ingest(input)).model_dump(mode="json")


async def content_read(runtime: Runtime, ref: str) -> dict[str, object]:
    async with runtime.execution.operation("content.read"):
        artifact = await runtime.artifacts.get(ref)
        blob = await runtime.artifacts.blob(ref)
        value: dict[str, object] = {
            "artifact": artifact.model_dump(mode="json"),
            "content": blob.model_dump(mode="json"),
        }
        if is_text_media_type(artifact.media_type):
            value["text"] = await runtime.artifacts.read_text(ref)
        else:
            value["export_hint"] = (
                f'nous tools content export "{artifact.ref.identifier}" --output PATH'
            )
        return value


async def content_export(
    runtime: Runtime,
    ref: str,
    output: Path,
    on_export: Callable[[Path, Path, str, int], Awaitable[None]] | None = None,
) -> dict[str, object]:
    async with runtime.execution.operation("content.export"):
        if on_export is None:
            await runtime.artifacts.export(ref, output)
        else:
            blob = await runtime.artifacts.blob(ref)
            async with runtime.artifacts.borrow_local(ref) as source:
                await on_export(source, output, blob.digest, blob.byte_length)
        return {"artifact": ref, "output": str(output)}


async def media_inspect(runtime: Runtime, input: InputSpec) -> dict[str, object]:
    async with runtime.execution.operation("media.inspect") as context:
        artifact = await runtime.artifacts.accept(input)
        info = await runtime.media.preparer.inspect(artifact, context.settings.values.media)
        return {"artifact": artifact.model_dump(mode="json"), "media": info}


async def media_analyze(
    runtime: Runtime,
    input: InputSpec,
    strategy: Literal["description_only", "direct_structured", "describe_then_structure"],
    selection: SourceSelection,
    question: str,
    instructions: str,
    task: str | None,
    description_task: str | None,
    document_mode: str,
    without_audio: bool,
    description_result: Sequence[str] | None,
    schema: dict[str, JsonValue] | None,
    details: bool,
) -> dict[str, object]:
    async with runtime.execution.operation("tools.media.analyze") as context:
        outcome = await runtime.media.analyze(
            input,
            strategy,
            context.settings,
            selection=selection,
            question=question,
            instructions=instructions,
            task=task,
            description_task=description_task,
            document_mode=document_mode,
            without_audio=without_audio,
            description_result=description_result,
            schema=schema,
        )
        return await _media_result(runtime, outcome, details)


async def _media_result(
    runtime: Runtime, outcome: AnalysisResult, details: bool
) -> dict[str, object]:
    return {
        "artifact": outcome.artifact.ref.identifier,
        "coverage": outcome.coverage,
        "descriptions": [item.ref.identifier for item in outcome.descriptions],
        "results": [
            await runtime.results.read_view(item.ref.identifier, sources=True, details=details)
            for item in outcome.batches
        ],
    }


async def media_recover(runtime: Runtime, ref: str, details: bool) -> dict[str, object]:
    async with runtime.execution.operation("tools.media.recover") as context:
        return await _media_result(
            runtime, await runtime.media.recover(ref, context.settings), details
        )


async def result_read(
    runtime: Runtime, ref: str, sources: bool, details: bool
) -> dict[str, object]:
    async with runtime.execution.operation("result.read"):
        return await runtime.results.read_view(ref, sources=sources, details=details)


async def embedding_probe(
    runtime: Runtime, model: str, inputs: Sequence[TextInput]
) -> dict[str, object]:
    result = await runtime.embedding.probe(model, inputs)
    return {
        "model": model,
        "dimensions": result.dimensions,
        "inputs": len(result.vectors),
        "producer": result.producer,
        "usage": result.usage,
    }


async def embedding_run(
    runtime: Runtime,
    space: str,
    purpose: Literal["document", "query"],
    inputs: Sequence[TextInput],
) -> dict[str, object]:
    return (await runtime.embedding.run(space, purpose, inputs)).model_dump(mode="json")
