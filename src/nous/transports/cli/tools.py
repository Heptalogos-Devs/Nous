"""Local model, prompt, media and durable content tools."""

from __future__ import annotations

import json
import math
from pathlib import Path
from typing import Annotated, Literal, cast

import click
import typer

from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.content.contracts import ContentReadView, InputSpec, SourceSelection, TextInput
from nous.models.prompts import PromptCatalog
from nous.runtime.bootstrap import task_catalog

from . import application_action, emit, emit_content_view, options
from .input_files import json_parameter
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


def _input(value: str) -> InputSpec:
    return (
        InputSpec(kind="artifact", artifact=value)
        if identifier_type(value) == "art"
        else InputSpec(kind="file", path=value)
    )


def _text_input(value: str) -> TextInput:
    return (
        TextInput(kind="artifact", artifact=value)
        if identifier_type(value) == "art"
        else TextInput(kind="file", path=value)
    )


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


def _emit_results(ctx: typer.Context, value: dict[str, object]) -> None:
    """Present named bodies and readable references; machine output keeps the complete value."""
    if options(ctx).json_mode:
        emit(ctx, value)
        return
    results = cast(list[dict[str, object]], value.get("results", [value]))
    for result in results:
        outputs = cast(list[dict[str, object]], result.get("outputs", []))
        visible = [output for output in outputs if output.get("name") != "response"]
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
    emit(ctx, cast(dict[str, object], application_action(ctx, "configuration.inspect", "models")))


@model_cli.command("discover")
def model_discover(ctx: typer.Context, provider: str) -> None:
    emit(ctx, cast(dict[str, object], application_action(ctx, "tools.model_discover", provider)))


@model_cli.command("probe")
def model_probe(
    ctx: typer.Context,
    model: str,
    input: Annotated[
        Path | None, typer.Option("--input", help="用于检查的真实媒体或文字文件")
    ] = None,
) -> None:
    emit(ctx, cast(dict[str, object], application_action(ctx, "tools.model_probe", model, input)))


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
    delivered = False

    def event(name: str, value: object) -> None:
        nonlocal delivered
        if name == "prepared":
            typer.echo(json.dumps(value, ensure_ascii=False), err=True)
        elif name == "delta" and isinstance(value, str):
            delivered = True
            if options(ctx).json_mode:
                typer.echo(json.dumps({"event": "delta", "text": value}, ensure_ascii=False))
            else:
                typer.echo(value, nl=False)
        elif name == "completed" and isinstance(value, str):
            if options(ctx).json_mode:
                typer.echo(json.dumps({"event": "completed", "result": value}, ensure_ascii=False))
            else:
                typer.echo(f"\n结果 / Result: {value}", err=True)
        else:
            raise ValueError("Unexpected model event")

    try:
        value = cast(
            dict[str, object],
            application_action(
                ctx,
                "tools.model_run",
                task,
                [_input(item) for item in input or ()],
                text or (),
                question,
                instructions,
                scope=None,
                schema=None,
                document_mode=document_mode,
                without_audio=without_audio,
                per_batch=per_batch,
                details=details or options(ctx).json_mode,
                stream=stream,
                show_request=show_request,
                on_event=event if stream or show_request else None,
                input_parameters=(json_parameter("schema", schema),) if schema else (),
            ),
        )
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
        if details:
            emit(ctx, value)
        else:
            _emit_results(ctx, value)


@model_cli.command("recover")
def model_recover(
    ctx: typer.Context,
    producer_key: Annotated[
        str, typer.Option("--producer-key", help="失败结果中提供的模型响应恢复键")
    ],
    details: Annotated[bool, typer.Option("--details")] = False,
) -> None:
    """恢复并保存一个已经成功生成的模型结果。"""

    value = cast(
        dict[str, object],
        application_action(
            ctx, "tools.model_recover", producer_key, details or options(ctx).json_mode
        ),
    )
    if details:
        emit(ctx, value)
    else:
        _emit_results(ctx, value)


@prompt_cli.command("list")
def prompt_list(ctx: typer.Context) -> None:
    emit(ctx, {"prompts": PromptCatalog(options(ctx).locator.directory, task_catalog()).list()})


@prompt_cli.command("show")
def prompt_show(
    ctx: typer.Context,
    prompt: str,
    origin: Annotated[
        Literal["selected", "package", "instance"], typer.Option("--origin")
    ] = "selected",
) -> None:
    emit(
        ctx,
        PromptCatalog(options(ctx).locator.directory, task_catalog())
        .get(prompt, origin=origin)
        .to_dict(),
    )


@prompt_cli.command("describe")
def prompt_describe(
    ctx: typer.Context,
    prompt: str,
    task: Annotated[str | None, typer.Option("--task")] = None,
) -> None:
    result = application_action(ctx, "configuration.inspect", "prompt", prompt, task=task)
    emit(ctx, cast(dict[str, object], result))


@prompt_cli.command("diff")
def prompt_diff(ctx: typer.Context, prompt: str) -> None:
    emit(ctx, PromptCatalog(options(ctx).locator.directory, task_catalog()).diff(prompt))


@prompt_cli.command("restore")
def prompt_restore(ctx: typer.Context, prompt: str) -> None:
    emit(
        ctx,
        PromptCatalog(options(ctx).locator.directory, task_catalog())
        .restore_package(prompt)
        .to_dict(),
    )


@prompt_cli.command("validate")
def prompt_validate(ctx: typer.Context, prompt: str) -> None:
    emit(ctx, PromptCatalog(options(ctx).locator.directory, task_catalog()).validate(prompt))


@prompt_cli.command("edit")
def prompt_edit(
    ctx: typer.Context,
    prompt: str,
    from_dir: Annotated[Path | None, typer.Option("--from-dir")] = None,
) -> None:
    catalog = PromptCatalog(options(ctx).locator.directory, task_catalog())
    if from_dir is not None:
        target = catalog.import_directory(prompt, from_dir)
    else:
        prepared = cast(dict[str, object], application_action(ctx, "prompt.prepare_edit", prompt))
        target = Path(str(prepared["directory"]))
        for filename in ("CONTEXT.md", "prompt.toml", *cast(list[str], prepared["templates"])):
            click.edit(filename=str(target / filename))
    emit(ctx, {"directory": str(target), **catalog.validate(prompt)})


@prompt_cli.command("import")
def prompt_import(ctx: typer.Context, prompt: str, source: Path) -> None:
    catalog = PromptCatalog(options(ctx).locator.directory, task_catalog())
    target = catalog.import_directory(prompt, source)
    application_action(ctx, "prompt.write_context", prompt)
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
    task: Annotated[str | None, typer.Option("--task")] = None,
    per_batch: Annotated[bool, typer.Option("--per-batch")] = False,
) -> None:
    result = application_action(
        ctx,
        "tools.prompt_preview",
        prompt,
        [_input(item) for item in input or ()],
        text or (),
        question,
        instructions,
        schema=None,
        task=task,
        per_batch=per_batch,
        input_parameters=(json_parameter("schema", schema),) if schema else (),
    )
    emit(ctx, cast(dict[str, object], result))


@content_cli.command("add")
def content_add(ctx: typer.Context, input: Path) -> None:
    emit(ctx, cast(dict[str, object], application_action(ctx, "tools.content_add", input)))


@content_cli.command("read")
def content_read(ctx: typer.Context, ref: str) -> None:
    emit(ctx, cast(dict[str, object], application_action(ctx, "tools.content_read", ref)))


@content_cli.command("export")
def content_export(
    ctx: typer.Context, ref: str, output: Annotated[Path, typer.Option("--output")]
) -> None:
    emit(ctx, cast(dict[str, object], application_action(ctx, "tools.content_export", ref, output)))


@media_cli.command("inspect")
def media_inspect(ctx: typer.Context, input: str) -> None:
    emit(
        ctx, cast(dict[str, object], application_action(ctx, "tools.media_inspect", _input(input)))
    )


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
    details: Annotated[bool, typer.Option("--details")] = False,
) -> None:
    selected = _selection(pages, start, end)
    value = cast(
        dict[str, object],
        application_action(
            ctx,
            "tools.media_analyze",
            _input(input),
            strategy,
            selection=selected,
            question=question,
            instructions=instructions,
            task=task,
            description_task=description_task,
            document_mode=document_mode,
            without_audio=without_audio,
            description_result=description_result,
            schema=None,
            details=details or options(ctx).json_mode,
            input_parameters=(json_parameter("schema", schema),) if schema else (),
        ),
    )
    if details:
        emit(ctx, value)
    else:
        _emit_results(ctx, value)


@media_cli.command("recover")
def media_recover(
    ctx: typer.Context, ref: str, details: Annotated[bool, typer.Option()] = False
) -> None:
    value = cast(
        dict[str, object],
        application_action(ctx, "tools.media_recover", ref, details or options(ctx).json_mode),
    )
    if details:
        emit(ctx, value)
    else:
        _emit_results(ctx, value)


@result_cli.command("read")
def result_read(
    ctx: typer.Context,
    ref: str,
    sources: Annotated[bool, typer.Option()] = False,
    details: Annotated[bool, typer.Option()] = False,
) -> None:
    value = cast(
        dict[str, object],
        application_action(
            ctx, "tools.result_read", ref, sources, details or options(ctx).json_mode
        ),
    )
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
    sources = [
        *(_text_input(item) for item in input or ()),
        *(TextInput(kind="text", text=item) for item in text or ()),
    ]
    value = application_action(ctx, "tools.embedding_probe", model, sources)
    emit(ctx, cast(dict[str, object], value))


@embedding_cli.command("run")
def embedding_run(
    ctx: typer.Context,
    space: Annotated[str, typer.Option("--space")],
    purpose: Annotated[Literal["document", "query"], typer.Option("--purpose")],
    input: Annotated[list[str] | None, typer.Option("--input")] = None,
    text: Annotated[list[str] | None, typer.Option("--text")] = None,
    details: Annotated[bool, typer.Option("--details")] = False,
) -> None:
    sources = [
        *(_text_input(item) for item in input or ()),
        *(TextInput(kind="text", text=item) for item in text or ()),
    ]
    value = cast(
        dict[str, object],
        application_action(ctx, "tools.embedding_run", space, purpose, sources),
    )
    if options(ctx).json_mode or details:
        emit(ctx, value)
        return
    definition = cast(dict[str, object], value["space"])
    rows = cast(list[dict[str, object]], value["rows"])
    typer.echo(f"{space}: {len(rows)} vectors, {definition['dimensions']} dimensions ({purpose})")
    for ref in cast(list[dict[str, str]], value["results"]):
        typer.echo(ref["identifier"])
