"""Local model, prompt, media and durable content tools."""

from __future__ import annotations

import asyncio
import json
import math
from collections.abc import Mapping
from importlib.resources.abc import Traversable
from pathlib import Path
from typing import Annotated, Literal, cast

import click
import typer

from nous.application.config_operations import LIST_MODELS, EmptyRequest
from nous.application.contracts import EventEnvelope, Saved, TextDelta
from nous.application.embedding import PROBE_EMBEDDING, RUN_EMBEDDING, ProbeEmbedding, RunEmbedding
from nous.application.inputs import FileInput, InputProvider, StreamInput
from nous.application.prompt_operations import (
    DESCRIBE_PROMPT,
    DIFF_PROMPT,
    IMPORT_PROMPT,
    LIST_PROMPTS,
    PREPARE_PROMPT_EDIT,
    READ_PROMPT,
    RESTORE_PROMPT,
    VALIDATE_PROMPT,
    DescribePrompt,
    ImportPrompt,
    ListPrompts,
    PromptImportView,
    PromptRequest,
    ReadPrompt,
)
from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.content.contracts import (
    AcceptContent,
    ArtifactInput,
    ContentReadView,
    ExportContent,
    ExternalInput,
    InlineText,
    ReadArtifact,
    ReadResult,
    SourceSelection,
    SourceTarget,
)
from nous.content.operations import ACCEPT_CONTENT, EXPORT_CONTENT, READ_ARTIFACT, READ_RESULT
from nous.media.analysis import ANALYZE_MEDIA, INSPECT_MEDIA, AnalyzeMedia, InspectMedia
from nous.models.generation.contracts import DiscoverModels, PreviewPrompt, ProbeModel, RunModel
from nous.models.generation.operations import MODEL_DISCOVER, MODEL_PROBE, MODEL_RUN, PROMPT_PREVIEW
from nous.models.prompts import PromptCatalog
from nous.runtime.bootstrap import task_catalog
from nous.runtime.execution import complete_cleanup

from .context import application_call, emit, emit_content_view, options, run_async
from .wave import wave_cli

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
    (wave_cli, "wave"),
):
    tools_cli.add_typer(group, name=name)


def _model_inputs(
    values: list[str] | tuple[str, ...],
    texts: list[str] | tuple[str, ...] = (),
) -> tuple[tuple[SourceTarget, ...], dict[str, InputProvider]]:
    sources: list[SourceTarget] = []
    bindings: dict[str, InputProvider] = {}
    for index, value in enumerate(values):
        if identifier_type(value) == "art":
            sources.append(ArtifactInput(artifact=value))
        else:
            key = f"input{index + 1}"
            sources.append(ExternalInput(key=key))
            bindings[key] = FileInput(Path(value))
    sources.extend(InlineText(text=value) for value in texts)
    return tuple(sources), bindings


def _result_views(ctx: typer.Context, refs: object, *, details: bool) -> dict[str, object]:
    from nous.content.contracts import ResultRef

    return {
        "results": [
            application_call(ctx, READ_RESULT, ReadResult(ref=ref, details=details)).model_dump(
                mode="json"
            )
            for ref in cast(tuple[ResultRef, ...], refs)
        ]
    }


def _schema(path: Path | None, bindings: dict[str, InputProvider]) -> ExternalInput | None:
    if path is None:
        return None
    bindings["output_schema"] = FileInput(path, name=path.name, source=str(path))
    return ExternalInput(key="output_schema")


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


def _selection(
    pages: str | None, start: float | None, end: float | None, crop: str | None = None
) -> SourceSelection:
    if any(
        value is not None and (not math.isfinite(value) or not math.isfinite(value * 1000))
        for value in (start, end)
    ):
        raise NousError("media.invalid_selection", "Media times must be finite")
    coordinates = None
    if crop is not None:
        try:
            values = tuple(int(value.strip()) for value in crop.split(","))
        except ValueError as error:
            raise NousError(
                "media.invalid_selection", "Crop requires left,top,right,bottom"
            ) from error
        if len(values) != 4:
            raise NousError("media.invalid_selection", "Crop requires left,top,right,bottom")
        coordinates = (values[0], values[1], values[2], values[3])
    return SourceSelection(
        pages=_pages(pages),
        start_ms=round(start * 1000) if start is not None else None,
        end_ms=round(end * 1000) if end is not None else None,
        crop=coordinates,
    )


def _independent_range(value: str, crop: str | None) -> SourceSelection:
    if value.startswith("pages:"):
        return _selection(value.removeprefix("pages:"), None, None, crop)
    first, separator, last = value.partition(":")
    if not separator:
        raise NousError("media.invalid_selection", "Use START:END seconds or pages:2-4,6")
    try:
        return _selection(None, float(first), float(last), crop)
    except ValueError as error:
        raise NousError(
            "media.invalid_selection", "Use START:END seconds or pages:2-4,6"
        ) from error


def _emit_results(ctx: typer.Context, value: dict[str, object]) -> None:
    """Present named bodies and readable references; machine output keeps the complete value."""
    if options(ctx).json_mode:
        emit(ctx, value)
        return
    results = cast(list[dict[str, object]], value.get("results", [value]))
    for result in results:
        outputs = cast(list[dict[str, object]], result.get("outputs", []))
        visible = [output for output in outputs if output.get("role") != "diagnostic"]
        for output in visible:
            if len(visible) > 1:
                typer.echo(f"\n{output['name']}")
            emit_content_view(ContentReadView.model_validate(output["view"]))
        for key, views in cast(dict[str, list[object]], result.get("source_views", {})).items():
            typer.echo(f"\n[{key}]")
            for view in views:
                emit_content_view(ContentReadView.model_validate(view))
        reference = cast(dict[str, object], result.get("ref", {})).get("identifier")
        if reference:
            typer.echo(f"\n{reference}")


@model_cli.command("list")
def model_list(ctx: typer.Context) -> None:
    emit(
        ctx,
        application_call(ctx, LIST_MODELS, EmptyRequest()),
    )


@model_cli.command("discover")
def model_discover(ctx: typer.Context, provider: str) -> None:
    emit(ctx, application_call(ctx, MODEL_DISCOVER, DiscoverModels(provider=provider)))


@model_cli.command("probe")
def model_probe(
    ctx: typer.Context,
    model: str,
    input: Annotated[
        Path | None, typer.Option("--input", help="用于检查的真实媒体或文字文件")
    ] = None,
) -> None:
    sources, bindings = _model_inputs([str(input)] if input else [])
    outcome = application_call(
        ctx,
        MODEL_PROBE,
        ProbeModel(model=model, source=sources[0] if sources else None),
        inputs=bindings,
    )
    _emit_results(ctx, _result_views(ctx, outcome.results, details=options(ctx).json_mode))


@model_cli.command("run")
def model_run(
    ctx: typer.Context,
    task: Annotated[str, typer.Option("--task")],
    input: Annotated[
        list[str] | None,
        typer.Option("--input", help='文件或 Artifact 地址（如 "art(...)"）；可重复'),
    ] = None,
    text: Annotated[
        list[str] | None, typer.Option("--text", help="文字输入；可重复，排在文件之后")
    ] = None,
    question: Annotated[str, typer.Option()] = "",
    instructions: Annotated[str, typer.Option()] = "",
    schema: Annotated[Path | None, typer.Option("--schema")] = None,
    show_request: Annotated[bool, typer.Option("--show-request")] = False,
    details: Annotated[bool, typer.Option("--details")] = False,
    stream: Annotated[bool, typer.Option("--stream")] = False,
    per_batch: Annotated[
        bool, typer.Option("--per-batch", help="每批独立回答；默认所有输入共同回答")
    ] = False,
    document_mode: Annotated[str, typer.Option()] = "auto",
    without_audio: Annotated[bool, typer.Option()] = False,
) -> None:
    sources, bindings = _model_inputs(input or [], text or [])

    def event(envelope: EventEnvelope) -> None:
        if isinstance(envelope.event, TextDelta):
            if options(ctx).json_mode:
                typer.echo(envelope.model_dump_json())
            else:
                typer.echo(envelope.event.text, nl=False)
        elif isinstance(envelope.event, Saved) and stream:
            if options(ctx).json_mode:
                typer.echo(envelope.model_dump_json())
            else:
                typer.echo(f"\n结果 / Result: {envelope.event.refs}", err=True)

    request = RunModel(
        task=task,
        sources=sources,
        question=question,
        instructions=instructions,
        output_schema=_schema(schema, bindings),
        input_mode="independent" if per_batch else "joint",
        document_mode=cast(Literal["auto", "text", "visual"], document_mode),
        without_audio=without_audio,
        stream=stream,
    )
    outcome = application_call(
        ctx, MODEL_RUN, request, inputs=bindings, observer=event if stream else None
    )
    value = _result_views(
        ctx, outcome.results, details=details or show_request or options(ctx).json_mode
    )
    if show_request:
        for item in cast(list[dict[str, object]], value["results"]):
            typer.echo(json.dumps(item.get("request"), ensure_ascii=False), err=True)
    if details or options(ctx).json_mode:
        emit(ctx, value)
    elif not stream:
        _emit_results(ctx, value)


@prompt_cli.command("list")
def prompt_list(ctx: typer.Context) -> None:
    emit(ctx, application_call(ctx, LIST_PROMPTS, ListPrompts()))


@prompt_cli.command("show")
def prompt_show(
    ctx: typer.Context,
    prompt: str,
    origin: Annotated[
        Literal["selected", "package", "instance"], typer.Option("--origin")
    ] = "selected",
) -> None:
    emit(ctx, application_call(ctx, READ_PROMPT, ReadPrompt(prompt=prompt, origin=origin)))


@prompt_cli.command("describe")
def prompt_describe(
    ctx: typer.Context,
    prompt: str,
    task: Annotated[str | None, typer.Option("--task")] = None,
) -> None:
    result = application_call(ctx, DESCRIBE_PROMPT, DescribePrompt(prompt=prompt, task=task))
    emit(ctx, result.model_dump(mode="json"))


@prompt_cli.command("diff")
def prompt_diff(ctx: typer.Context, prompt: str) -> None:
    emit(ctx, application_call(ctx, DIFF_PROMPT, PromptRequest(prompt=prompt)))


@prompt_cli.command("restore")
def prompt_restore(ctx: typer.Context, prompt: str) -> None:
    emit(ctx, application_call(ctx, RESTORE_PROMPT, PromptRequest(prompt=prompt)))


@prompt_cli.command("validate")
def prompt_validate(ctx: typer.Context, prompt: str) -> None:
    emit(ctx, application_call(ctx, VALIDATE_PROMPT, PromptRequest(prompt=prompt)))


def _import_prompt(ctx: typer.Context, prompt: str, source: Path) -> PromptImportView:
    async def prepare(
        request: ImportPrompt, bindings: Mapping[str, InputProvider], limit: int
    ) -> tuple[ImportPrompt, Mapping[str, InputProvider]]:
        def select() -> tuple[ImportPrompt, Mapping[str, InputProvider]]:
            total = 0

            def read(path: Traversable) -> str:
                nonlocal total
                with Path(str(path)).open("rb") as stream:
                    data = stream.read(limit - total + 1)
                total += len(data)
                if total > limit:
                    raise NousError(
                        "content.too_large", "Prompt input exceeds the configured read limit"
                    )
                return data.decode("utf-8")

            if not source.is_dir():
                raise NousError("prompt.invalid_source", "Prompt source must be a directory")
            catalog = PromptCatalog(options(ctx).locator.directory, task_catalog())
            resource = catalog.read_directory(prompt, source, read_text=read)
            contents = {"prompt.toml": resource.manifest_text, **resource.templates}
            files: dict[str, ExternalInput] = {}
            selected = dict(bindings)
            for index, (name, text) in enumerate(contents.items()):
                key = f"prompt{index}"
                files[name] = ExternalInput(key=key)
                selected[key] = StreamInput(
                    (text.encode("utf-8"),), name=Path(name).name, source=str(source / name)
                )
            return request.model_copy(update={"files": files}), selected

        return await complete_cleanup(asyncio.to_thread(select))

    return application_call(
        ctx, IMPORT_PROMPT, ImportPrompt(prompt=prompt, files={}), prepare=prepare
    )


@prompt_cli.command("edit")
def prompt_edit(
    ctx: typer.Context,
    prompt: str,
    from_dir: Annotated[Path | None, typer.Option("--from-dir")] = None,
) -> None:
    if from_dir is not None:
        emit(ctx, _import_prompt(ctx, prompt, from_dir))
        return
    else:
        prepared = application_call(ctx, PREPARE_PROMPT_EDIT, PromptRequest(prompt=prompt))
        target = Path(str(prepared["directory"]))
        for filename in ("CONTEXT.md", "prompt.toml", *prepared["templates"]):
            click.edit(filename=str(target / filename))
    emit(
        ctx,
        {
            "directory": str(target),
            **application_call(ctx, VALIDATE_PROMPT, PromptRequest(prompt=prompt)),
        },
    )


@prompt_cli.command("import")
def prompt_import(ctx: typer.Context, prompt: str, source: Path) -> None:
    emit(ctx, _import_prompt(ctx, prompt, source))


@prompt_cli.command("preview")
def prompt_preview(
    ctx: typer.Context,
    prompt: str,
    input: Annotated[list[str] | None, typer.Option("--input")] = None,
    text: Annotated[list[str] | None, typer.Option("--text")] = None,
    question: Annotated[str, typer.Option()] = "",
    instructions: Annotated[str, typer.Option()] = "",
    schema: Annotated[Path | None, typer.Option("--schema")] = None,
    task: Annotated[str | None, typer.Option("--task")] = None,
    per_batch: Annotated[bool, typer.Option("--per-batch")] = False,
) -> None:
    sources, bindings = _model_inputs(input or [], text or [])
    result = application_call(
        ctx,
        PROMPT_PREVIEW,
        PreviewPrompt(
            prompt=prompt,
            sources=sources,
            question=question,
            instructions=instructions,
            output_schema=_schema(schema, bindings),
            task=task,
            input_mode="independent" if per_batch else "joint",
        ),
        inputs=bindings,
    )
    emit(ctx, result)


@content_cli.command("add")
def content_add(ctx: typer.Context, input: Path) -> None:
    artifact = application_call(
        ctx, ACCEPT_CONTENT, AcceptContent(key="content"), inputs={"content": FileInput(input)}
    )
    emit(ctx, artifact.model_dump(mode="json"))


@content_cli.command("read")
def content_read(ctx: typer.Context, ref: str) -> None:
    view = application_call(ctx, READ_ARTIFACT, ReadArtifact(ref=ref))
    if options(ctx).json_mode:
        emit(ctx, view.model_dump(mode="json"))
    else:
        emit_content_view(view)


@content_cli.command("export")
def content_export(
    ctx: typer.Context, ref: str, output: Annotated[Path, typer.Option("--output")]
) -> None:
    from nous.runtime.bootstrap import open_instance

    from .local_control import request_export

    offer = application_call(ctx, EXPORT_CONTENT, ExportContent(ref=ref))
    if not request_export(options(ctx).locator, offer.artifact_ref, output):

        async def local_export() -> None:
            async with open_instance(options(ctx).locator) as runtime:
                await runtime.artifacts.export(offer.artifact_ref, output)

        run_async(local_export())
    emit(ctx, {"artifact": offer.artifact_ref.identifier, "output": str(output)})


@media_cli.command("inspect")
def media_inspect(ctx: typer.Context, input: str) -> None:
    sources, bindings = _model_inputs([input])
    result = application_call(ctx, INSPECT_MEDIA, InspectMedia(source=sources[0]), inputs=bindings)
    emit(ctx, result.model_dump(mode="json"))


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
    crop: Annotated[
        str | None, typer.Option("--crop", help="left,top,right,bottom in pixels")
    ] = None,
    task: Annotated[str | None, typer.Option()] = None,
    description_task: Annotated[str | None, typer.Option()] = None,
    description_result: Annotated[list[str] | None, typer.Option()] = None,
    question: Annotated[str, typer.Option()] = "",
    instructions: Annotated[str, typer.Option()] = "",
    document_mode: Annotated[str, typer.Option()] = "auto",
    without_audio: Annotated[bool, typer.Option()] = False,
    per_batch: Annotated[bool, typer.Option("--per-batch", help="独立分析所选范围")] = False,
    independent_range: Annotated[
        list[str] | None,
        typer.Option("--range", help="独立范围：秒数 START:END 或 pages:2-4,6；可重复"),
    ] = None,
    schema: Annotated[Path | None, typer.Option("--schema")] = None,
    details: Annotated[bool, typer.Option("--details")] = False,
) -> None:
    selected = _selection(pages, start, end, crop)
    sources, bindings = _model_inputs([input])
    if len(description_result or ()) > 1:
        raise NousError("media.description_mismatch", "Choose one saved description")
    target = sources[0]
    if isinstance(target, (ArtifactInput, ExternalInput)):
        target = target.model_copy(update={"selection": selected})
    outcome = application_call(
        ctx,
        ANALYZE_MEDIA,
        AnalyzeMedia(
            source=target,
            strategy=strategy,
            question=question,
            instructions=instructions,
            structure_task=task,
            description_task=description_task,
            description=description_result[0] if description_result else None,
            document_mode=cast(Literal["auto", "text", "visual"], document_mode),
            input_mode="independent" if per_batch or independent_range else "joint",
            independent_ranges=tuple(
                _independent_range(value, crop) for value in independent_range or ()
            ),
            without_audio=without_audio,
            output_schema=_schema(schema, bindings),
        ),
        inputs=bindings,
    )
    value = _result_views(
        ctx, outcome.results or outcome.descriptions, details=details or options(ctx).json_mode
    )
    value["original"] = outcome.original.model_dump(mode="json")
    value["descriptions"] = [item.model_dump(mode="json") for item in outcome.descriptions]
    value["coverage"] = outcome.coverage
    if details:
        emit(ctx, value)
    else:
        _emit_results(ctx, value)


@result_cli.command("read")
def result_read(
    ctx: typer.Context,
    ref: str,
    sources: Annotated[bool, typer.Option("--sources/--no-sources")] = True,
    details: Annotated[bool, typer.Option()] = False,
) -> None:
    result = application_call(
        ctx,
        READ_RESULT,
        ReadResult(ref=ref, sources=sources, details=details or options(ctx).json_mode),
    )
    value = result.model_dump(mode="json")
    if details:
        emit(ctx, value)
    else:
        _emit_results(ctx, value)


@embedding_cli.command("probe")
def embedding_probe(
    ctx: typer.Context,
    model: str,
    input: Annotated[list[str] | None, typer.Option("--input")] = None,
    text: Annotated[list[str] | None, typer.Option("--text")] = None,
) -> None:
    sources, bindings = _model_inputs(input or [], text or [])
    value = application_call(
        ctx, PROBE_EMBEDDING, ProbeEmbedding(model=model, sources=sources), inputs=bindings
    )
    emit(ctx, value.model_dump(mode="json"))


@embedding_cli.command("run")
def embedding_run(
    ctx: typer.Context,
    space: Annotated[str, typer.Option("--space")],
    purpose: Annotated[Literal["document", "query"], typer.Option("--purpose")],
    input: Annotated[list[str] | None, typer.Option("--input")] = None,
    text: Annotated[list[str] | None, typer.Option("--text")] = None,
    details: Annotated[bool, typer.Option("--details")] = False,
) -> None:
    sources, bindings = _model_inputs(input or [], text or [])
    outcome = application_call(
        ctx,
        RUN_EMBEDDING,
        RunEmbedding(space=space, purpose=purpose, sources=sources),
        inputs=bindings,
    )
    value = outcome.model_dump(mode="json")
    if options(ctx).json_mode or details:
        emit(ctx, value)
        return
    definition = cast(dict[str, object], value["space"])
    rows = cast(list[dict[str, object]], value["rows"])
    typer.echo(f"{space}: {len(rows)} vectors, {definition['dimensions']} dimensions ({purpose})")
    for ref in cast(list[dict[str, str]], value["results"]):
        typer.echo(ref["identifier"])
