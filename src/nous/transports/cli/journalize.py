"""Writing and draft adoption commands over the shared Journalize operation."""

import sys
from pathlib import Path
from typing import Annotated

import typer

from nous.cognition.contracts import JournalizeOutcome
from nous.cognition.retrieval import Mode
from nous.common.errors import NousError
from nous.runtime.bootstrap import open_instance

from . import cli, options, run_async
from .cognition import emit_record, journal_cli, read_body, select_source_bindings

draft_cli = typer.Typer(help="读取和采用草稿 / Journal drafts", no_args_is_help=True)
journal_cli.add_typer(draft_cli, name="draft")


def emit_outcome(ctx: typer.Context, result: JournalizeOutcome) -> None:
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    elif result.record:
        emit_record(ctx, result.record)
        if result.draft_ref:
            typer.echo(f"草稿 / Draft: {result.draft_ref.address}")
    elif result.draft:
        typer.echo(f"{result.draft.ref.address}\n\n{result.draft.body}")
    else:
        typer.echo("没有新增内容 / No new material")


@cli.command("journalize")
def journalize(
    ctx: typer.Context,
    source: Annotated[
        list[str] | None,
        typer.Option(
            "--source", help="Selected Note, Journal or readable artifact; repeat for joint writing"
        ),
    ] = None,
    topic: str | None = None,
    purpose: str = "record",
    author: str | None = None,
    viewpoint: str | None = None,
    target: str | None = None,
    save: Annotated[bool | None, typer.Option("--save/--draft")] = None,
    namespace: str | None = None,
    stream: bool = False,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:

            def delta(text: str) -> None:
                typer.echo(text, nl=False, err=True)

            try:
                return await runtime.journalize.run(
                    namespace,
                    sources=await select_source_bindings(
                        runtime.records, namespace, source, current_basis, inspiration
                    ),
                    topic=topic,
                    purpose=purpose,
                    author=author,
                    viewpoint=viewpoint,
                    target_revision=target,
                    adoption=("save" if save else "draft") if save is not None else None,
                    on_delta=delta if stream else None,
                    refresh=refresh,
                    refresh_mode=refresh_mode,
                )
            finally:
                if stream:
                    typer.echo(err=True)

    emit_outcome(ctx, run_async(run()))


@draft_cli.command("read")
def draft_read(ctx: typer.Context, ref: str, namespace: str | None = None, sources: bool = False):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.journalize.read_draft(ref, namespace=namespace, sources=sources)

    result = run_async(run())
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    else:
        typer.echo(f"{result.ref.address}\n\n{result.body}")
        if result.adopted_revision:
            typer.echo(f"已采用 / Adopted: {result.adopted_revision.address}")
        for key, text in result.source_texts.items():
            typer.echo(f"\n[{key}]\n{text}")


@draft_cli.command("adopt")
def draft_adopt(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    text: str | None = None,
    file: Path | None = None,
    title: str | None = None,
    target: str | None = None,
    fork: bool = False,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    current_basis: Annotated[
        list[str] | None,
        typer.Option("--current-basis", help="Existing citation key to mark as current basis"),
    ] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    reference: Annotated[list[str] | None, typer.Option("--reference")] = None,
):
    if text is not None or file is not None:
        body = read_body(text, file)
    elif not sys.stdin.isatty():
        body = sys.stdin.read() or None
    else:
        body = None

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            bindings = None
            if current_basis or inspiration or reference:
                draft = await runtime.journalize.read_draft(ref, namespace=namespace)
                changes = (
                    [(key, "current_basis") for key in current_basis or ()]
                    + [(key, "inspiration") for key in inspiration or ()]
                    + [(key, "reference") for key in reference or ()]
                )
                known = {binding.key for binding in draft.context.source_bindings}
                if len({key for key, _ in changes}) != len(changes):
                    raise NousError(
                        "input.conflicting_sources",
                        "Each citation key can have only one explicit usage",
                    )
                if {key for key, _ in changes} - known:
                    raise NousError(
                        "input.unknown_source", "Usage changes must name existing citation keys"
                    )
                overrides = dict(changes)
                bindings = tuple(
                    binding.model_copy(update={"usage": overrides.get(binding.key, binding.usage)})
                    for binding in draft.context.source_bindings
                )
            return await runtime.journalize.adopt_draft(
                ref,
                namespace=namespace,
                body=body,
                title=title,
                target_revision=target,
                fork=fork,
                sources=bindings,
                refresh=refresh,
                refresh_mode=refresh_mode,
            )

    emit_record(ctx, run_async(run()))
