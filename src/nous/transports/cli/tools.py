"""Local model, prompt, media and durable content tools."""

from __future__ import annotations

import json
import math
from pathlib import Path
from typing import Annotated, Literal, cast

import click
import typer
from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot, Configuration, FrozenValues
from nous.content.contracts import ProcessingResult, SourceSelection
from nous.models.contracts import PreparedSource, ReadingInput
from nous.models.operations import PreparedRun
from nous.models.prompts import PromptCatalog, PromptCompiler
from nous.runtime.bootstrap import Runtime, open_instance

from . import emit, options, run_async

tools_cli = typer.Typer(help="模型、Prompt、媒体与内容工具", no_args_is_help=True)
model_cli = typer.Typer(help="模型资源与生成", no_args_is_help=True)
prompt_cli = typer.Typer(help="Prompt 资源", no_args_is_help=True)
content_cli = typer.Typer(help="接纳与读取内容", no_args_is_help=True)
media_cli = typer.Typer(help="查看与解释媒体", no_args_is_help=True)
result_cli = typer.Typer(help="读取处理结果", no_args_is_help=True)
embedding_cli = typer.Typer(help="文本向量", no_args_is_help=True)
for group, name in (
    (model_cli, "model"),
    (prompt_cli, "prompt"),
    (content_cli, "content"),
    (media_cli, "media"),
    (result_cli, "result"),
    (embedding_cli, "embedding"),
):
    tools_cli.add_typer(group, name=name)


def _source(value: str) -> Path | str:
    return value if value.startswith("art:") else Path(value)


def _schema(path: Path | None) -> dict[str, JsonValue] | None:
    if path is None:
        return None
    return TypeAdapter(dict[str, JsonValue]).validate_json(path.read_bytes())


def _pages(value: str | None) -> tuple[int, ...]:
    if not value:
        return ()
    pages: list[int] = []
    try:
        for part in value.split(","):
            first, separator, last = part.strip().partition("-")
            start = int(first)
            end = int(last) if separator else start
            if start < 1 or end < start:
                raise ValueError
            pages.extend(range(start, end + 1))
    except ValueError as error:
        raise NousError(
            "media.invalid_selection", "Pages must be 1-based numbers or ranges, for example 2-4,6"
        ) from error
    return tuple(dict.fromkeys(pages))


def _selection(pages: str | None, start: float | None, end: float | None) -> SourceSelection:
    if any(
        value is not None and (not math.isfinite(value) or not math.isfinite(value * 1000))
        for value in (start, end)
    ):
        raise NousError("media.invalid_selection", "Media times must be finite")
    return SourceSelection(
        pages=_pages(pages),
        start_ms=round(start * 1000) if start is not None else None,
        end_ms=round(end * 1000) if end is not None else None,
    )


async def _result(
    runtime: Runtime, address: str, *, sources: bool = True, details: bool = False
) -> dict[str, object]:
    result = await runtime.results.get(address)
    value = result.model_dump(mode="json")
    if not sources:
        value.pop("inputs", None)
    if not details:
        value.pop("request", None)
    rendered: list[dict[str, object]] = []
    for output in result.outputs:
        artifact = await runtime.artifacts.get(output.artifact)
        blob = await runtime.artifacts.blob(output.artifact)
        item: dict[str, object] = {
            "name": output.name,
            "artifact": artifact.ref.address,
            "media_type": artifact.media_type,
            "byte_length": blob.byte_length,
        }
        if output.name != "response" and (
            artifact.media_type.startswith("text/") or artifact.media_type == "application/json"
        ):
            item["text"] = await runtime.artifacts.read_text(output.artifact)
        rendered.append(item)
    value["outputs"] = rendered
    return value


@model_cli.command("list")
def model_list(ctx: typer.Context) -> None:
    settings = Configuration(options(ctx).locator.config_file).snapshot.typed_values()
    emit(
        ctx,
        {"models": {key: value.model_dump(mode="json") for key, value in settings.models.items()}},
    )


@model_cli.command("discover")
def model_discover(ctx: typer.Context, provider: str) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("model.discover") as context:
                names = await runtime.models.discover(provider, context.settings)
                return {"provider": provider, "models": names}

    emit(ctx, run_async(run()))


@model_cli.command("probe")
def model_probe(
    ctx: typer.Context,
    model: str,
    input: Annotated[
        Path | None, typer.Option("--input", help="用于检查的真实媒体或文字文件")
    ] = None,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            snapshot = runtime.configuration.snapshot
            values = snapshot.to_dict()
            if model not in snapshot.typed_values().models:
                raise NousError(
                    "model.unknown", "The model is not configured", details={"model": model}
                )
            tasks = cast(dict[str, object], values["tasks"])
            tasks["probe-internal"] = {
                "model": model,
                "prompt": "describe-media",
                "output_mode": "text",
            }
            from nous.configuration.definitions import NousConfig

            validated = NousConfig.model_validate(values).model_dump(mode="json")
            probe = ConfigSnapshot(
                FrozenValues(validated), snapshot.version, snapshot.sources, snapshot.state
            )
            outcome = await runtime.generation.run_input(
                "probe-internal",
                [input] if input else [],
                probe,
                texts=() if input else ("Describe the visible or supplied content briefly.",),
            )
            return {
                "model": model,
                "checked": "supplied_input" if input else "text",
                "results": [
                    await _result(runtime, result.ref.address) for result in outcome.results
                ],
                "coverage": outcome.coverage,
            }

    emit(ctx, run_async(run()))


@model_cli.command("run")
def model_run(
    ctx: typer.Context,
    task: Annotated[str, typer.Option("--task")],
    input: Annotated[
        list[str] | None, typer.Option("--input", help="文件或 art: 引用；可重复")
    ] = None,
    text: Annotated[
        list[str] | None, typer.Option("--text", help="文字输入；可重复，排在文件之后")
    ] = None,
    question: Annotated[str, typer.Option()] = "",
    instructions: Annotated[str, typer.Option()] = "",
    schema: Annotated[Path | None, typer.Option("--schema")] = None,
    show_request: Annotated[bool, typer.Option("--show-request")] = False,
    stream: Annotated[bool, typer.Option("--stream")] = False,
    document_mode: Annotated[str, typer.Option()] = "auto",
    without_audio: Annotated[bool, typer.Option()] = False,
) -> None:
    delivered = False

    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:

            def show(prepared: PreparedRun) -> None:
                for batch in prepared.batches:
                    typer.echo(json.dumps(batch.prepared.to_dict(), ensure_ascii=False), err=True)

            def delta(value: str) -> None:
                nonlocal delivered
                delivered = True
                if options(ctx).json_mode:
                    typer.echo(json.dumps({"event": "delta", "text": value}, ensure_ascii=False))
                else:
                    typer.echo(value, nl=False)

            def completed(result: ProcessingResult) -> None:
                if options(ctx).json_mode:
                    typer.echo(
                        json.dumps(
                            {"event": "completed", "result": result.ref.address},
                            ensure_ascii=False,
                        )
                    )
                else:
                    typer.echo(f"\n结果 / Result: {result.ref.address}", err=True)

            outcome = await runtime.generation.run_input(
                task,
                [_source(item) for item in input or ()],
                runtime.configuration.snapshot,
                question=question,
                instructions=instructions,
                schema=_schema(schema),
                document_mode=document_mode,
                without_audio=without_audio,
                texts=text or (),
                on_prepared=show if show_request else None,
                on_delta=delta if stream else None,
                on_completed=completed if stream else None,
            )
            return {
                "artifacts": [a.ref.address for a in outcome.artifacts],
                "results": [
                    await _result(runtime, result.ref.address) for result in outcome.results
                ],
                "coverage": outcome.coverage,
            }

    try:
        value = run_async(run())
    except (NousError, OSError, ValueError) as error:
        if stream and options(ctx).json_mode:
            failure = (
                error
                if isinstance(error, NousError)
                else NousError(
                    "operation.timeout" if isinstance(error, TimeoutError) else "operation.failed",
                    str(error)
                    or (
                        "Operation exceeded its deadline."
                        if isinstance(error, TimeoutError)
                        else "Operation failed."
                    ),
                    cause=error,
                )
            )
            failure.details["partial_output_delivered"] = delivered
            typer.echo(
                json.dumps({"event": "failed", "error": failure.to_dict()}, ensure_ascii=False)
            )
            raise SystemExit(130 if failure.code == "operation.cancelled" else 4) from None
        raise
    if not stream:
        emit(ctx, value)


@prompt_cli.command("list")
def prompt_list(ctx: typer.Context) -> None:
    emit(ctx, {"prompts": PromptCatalog(options(ctx).locator.directory).list()})


@prompt_cli.command("show")
def prompt_show(ctx: typer.Context, prompt: str) -> None:
    emit(ctx, PromptCatalog(options(ctx).locator.directory).get(prompt).to_dict())


@prompt_cli.command("validate")
def prompt_validate(ctx: typer.Context, prompt: str) -> None:
    emit(ctx, PromptCatalog(options(ctx).locator.directory).validate(prompt))


@prompt_cli.command("edit")
def prompt_edit(
    ctx: typer.Context,
    prompt: str,
    from_dir: Annotated[Path | None, typer.Option("--from-dir")] = None,
) -> None:
    catalog = PromptCatalog(options(ctx).locator.directory)
    if from_dir is not None:
        target = catalog.import_directory(prompt, from_dir)
    else:
        target = catalog.editable_directory(prompt)
        resource = catalog.get(prompt)
        for filename in ("prompt.toml", *resource.templates):
            click.edit(filename=str(target / filename))
    emit(ctx, {"directory": str(target), **catalog.validate(prompt)})


@prompt_cli.command("preview")
def prompt_preview(
    ctx: typer.Context,
    prompt: str,
    input: Annotated[list[str] | None, typer.Option("--input")] = None,
    text: Annotated[list[str] | None, typer.Option("--text")] = None,
    question: Annotated[str, typer.Option()] = "",
    instructions: Annotated[str, typer.Option()] = "",
    schema: Annotated[Path | None, typer.Option("--schema")] = None,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("prompt.preview") as context:
                sources: list[PreparedSource] = []
                for index, item in enumerate(input or (), 1):
                    original = (
                        await runtime.artifacts.get(item)
                        if item.startswith("art:")
                        else await runtime.artifacts.ingest(Path(item))
                    )
                    prepared = await runtime.media.preparer.prepare(
                        original,
                        requirements=(
                            "image/png",
                            "image/jpeg",
                            "image/webp",
                            "audio/wav",
                            "application/pdf",
                        ),
                        settings=context.settings.typed_values().media,
                        snapshot=context.settings,
                        context=context,
                    )
                    for batch in prepared.batches:
                        for source in batch.sources:
                            from dataclasses import replace

                            sources.append(replace(source, key=f"S{index}" + source.key[2:]))
                for index, value in enumerate(text or (), len(input or ()) + 1):
                    sources.append(PreparedSource(f"S{index}", f"text-{index}", text=value))
                compiled = PromptCompiler(runtime.prompts).compile(
                    prompt,
                    ReadingInput(question, instructions, tuple(sources)),
                    schema=_schema(schema),
                )
                rendered: dict[str, object] = dict(compiled.to_dict())
                return rendered

    emit(ctx, run_async(run()))


@content_cli.command("add")
def content_add(ctx: typer.Context, input: Path) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("content.add"):
                return (await runtime.artifacts.ingest(input)).model_dump(mode="json")

    emit(ctx, run_async(run()))


@content_cli.command("read")
def content_read(ctx: typer.Context, ref: str) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("content.read"):
                artifact = await runtime.artifacts.get(ref)
                blob = await runtime.artifacts.blob(ref)
                value: dict[str, object] = {
                    "artifact": artifact.model_dump(mode="json"),
                    "content": blob.model_dump(mode="json"),
                }
                if (
                    artifact.media_type.startswith("text/")
                    or artifact.media_type == "application/json"
                ):
                    value["text"] = await runtime.artifacts.read_text(ref)
                else:
                    value["export_hint"] = (
                        f"nous tools content export {artifact.ref.address} --output PATH"
                    )
                return value

    emit(ctx, run_async(run()))


@content_cli.command("export")
def content_export(
    ctx: typer.Context, ref: str, output: Annotated[Path, typer.Option("--output")]
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("content.export"):
                return {"artifact": ref, "output": str(await runtime.artifacts.export(ref, output))}

    emit(ctx, run_async(run()))


@media_cli.command("inspect")
def media_inspect(ctx: typer.Context, input: str) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("media.inspect") as context:
                artifact = (
                    await runtime.artifacts.get(input)
                    if input.startswith("art:")
                    else await runtime.artifacts.ingest(Path(input))
                )
                info = await runtime.media.preparer.inspect(
                    artifact, context.settings.typed_values().media
                )
                return {"artifact": artifact.model_dump(mode="json"), "media": info}

    emit(ctx, run_async(run()))


@media_cli.command("analyze")
def media_analyze(
    ctx: typer.Context,
    input: str,
    strategy: Annotated[
        Literal["description_only", "direct_structured", "describe_then_structure"], typer.Option()
    ],
    pages: Annotated[str | None, typer.Option()] = None,
    start: Annotated[float | None, typer.Option("--from", min=0)] = None,
    end: Annotated[float | None, typer.Option("--to", min=0)] = None,
    task: Annotated[str | None, typer.Option()] = None,
    description_task: Annotated[str | None, typer.Option()] = None,
    description_result: Annotated[list[str] | None, typer.Option()] = None,
    question: Annotated[str, typer.Option()] = "",
    instructions: Annotated[str, typer.Option()] = "",
    document_mode: Annotated[str, typer.Option()] = "auto",
    without_audio: Annotated[bool, typer.Option()] = False,
    schema: Annotated[Path | None, typer.Option("--schema")] = None,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("tools.media.analyze") as context:
                outcome = await runtime.media.analyze(
                    _source(input),
                    strategy,
                    context.settings,
                    selection=_selection(pages, start, end),
                    question=question,
                    instructions=instructions,
                    task=task,
                    description_task=description_task,
                    document_mode=document_mode,
                    without_audio=without_audio,
                    description_result=description_result,
                    schema=_schema(schema),
                )
                return {
                    "artifact": outcome.artifact.ref.address,
                    "coverage": outcome.coverage,
                    "descriptions": [item.ref.address for item in outcome.descriptions],
                    "results": [
                        await _result(runtime, item.ref.address) for item in outcome.batches
                    ],
                }

    emit(ctx, run_async(run()))


@result_cli.command("read")
def result_read(
    ctx: typer.Context,
    ref: str,
    sources: Annotated[bool, typer.Option()] = False,
    details: Annotated[bool, typer.Option()] = False,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            async with runtime.execution.operation("result.read"):
                return await _result(runtime, ref, sources=sources, details=details)

    emit(ctx, run_async(run()))


@embedding_cli.command("probe")
def embedding_probe(
    ctx: typer.Context,
    model: str,
    input: Annotated[list[str] | None, typer.Option("--input")] = None,
    text: Annotated[list[str] | None, typer.Option("--text")] = None,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            result = await runtime.embedding.probe(model, input or (), text or ())
            return {
                "model": model,
                "dimensions": result.dimensions,
                "inputs": len(result.vectors),
                "producer": result.producer,
                "usage": result.usage,
            }

    emit(ctx, run_async(run()))


@embedding_cli.command("run")
def embedding_run(
    ctx: typer.Context,
    space: Annotated[str, typer.Option("--space")],
    purpose: Annotated[Literal["document", "query"], typer.Option("--purpose")],
    input: Annotated[list[str] | None, typer.Option("--input")] = None,
    text: Annotated[list[str] | None, typer.Option("--text")] = None,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            result = await runtime.embedding.run(space, purpose, input or (), text or ())
            return await _result(runtime, result.ref.address)

    emit(ctx, run_async(run()))
