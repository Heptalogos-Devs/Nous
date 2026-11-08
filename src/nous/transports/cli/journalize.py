"""Writing and draft adoption commands over the shared Journalize operation."""

import sys
from pathlib import Path
from typing import Annotated, Literal

import typer
from pydantic import TypeAdapter

from nous.cognition.retrieval.contracts import Mode
from nous.cognition.writing.contracts import DraftOverride, SourceChanges, WritingOutcome
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection
from nous.content.text import BlockRef
from nous.presentation import translate
from nous.runtime.bootstrap import open_instance

from . import cli, language_for, options, run_async
from .cognition import after_write, emit_record, journal_cli, note_cli, read_body, source_requests
from .input_files import input_bytes

draft_cli = typer.Typer(help="Journal drafts", no_args_is_help=True)
journal_cli.add_typer(draft_cli, name="draft")


def emit_outcome(ctx: typer.Context, result: WritingOutcome) -> None:
    language = language_for(options(ctx))
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    elif result.record:
        if result.draft_ref:
            typer.echo(f"{translate('Draft', language)}: {result.draft_ref.identifier}")
        emit_record(ctx, result.record)
    elif result.draft:
        typer.echo(result.draft.ref.identifier)
        if result.draft.diff is not None:
            typer.echo(result.draft.diff or translate("No changes", language))
        else:
            typer.echo(result.draft.body)
    else:
        typer.echo(translate("No new material", language))
    if result.record is not None and result.record.status == "partial":
        raise typer.Exit(3)


@cli.command("journalize")
def journalize(
    ctx: typer.Context,
    source: Annotated[
        list[str] | None,
        typer.Option(
            "--source",
            help=(
                'Note or original material not adopted as a Journal, e.g. "note(...)"; '
                "repeat for joint writing"
            ),
        ),
    ] = None,
    topic: str | None = None,
    purpose: str = "record",
    author: str | None = None,
    viewpoint: str | None = None,
    save: Annotated[bool | None, typer.Option("--save/--draft")] = None,
    namespace: str | None = None,
    stream: bool = False,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:

            def delta(text: str) -> None:
                typer.echo(text, nl=False, err=True)

            try:
                services = await runtime.cognition(namespace)
                return await services.writing.journalize(
                    sources=source_requests(source, current_basis, inspiration),
                    topic=topic,
                    purpose=purpose,
                    author=author,
                    viewpoint=viewpoint,
                    adoption=("save" if save else "draft") if save is not None else None,
                    on_delta=delta if stream else None,
                    after=after_write(refresh, refresh_mode, background, inline, annotate),
                )
            finally:
                if stream:
                    typer.echo(err=True)

    emit_outcome(ctx, run_async(run()))


@draft_cli.command("read")
def draft_read(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    sources: bool = False,
    full_body: Annotated[bool, typer.Option("--body")] = False,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).writing.read_draft(
                ref, sources=sources
            )

    result = run_async(run())
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    else:
        typer.echo(result.ref.identifier)
        if result.adopted_as == "override":
            typer.echo(translate("Generated proposal", language_for(options(ctx))))
        typer.echo(
            result.body
            if full_body or result.diff is None
            else (result.diff or translate("No changes", language_for(options(ctx))))
        )
        if result.adopted_revision:
            typer.echo(
                f"{translate('Adopted as complete Markdown override' if result.adopted_as == 'override' else 'Adopted', language_for(options(ctx)))}: {result.adopted_revision.identifier}"
            )
        from . import emit_content_view

        for key, views in result.source_views.items():
            typer.echo(f"\n[{key}]")
            for view in views:
                emit_content_view(view)


@draft_cli.command("recover")
def draft_recover(ctx: typer.Context, request: str, namespace: str | None = None):
    """Continue writing from a previously saved request reference."""

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).writing.recover(request)

    emit_outcome(ctx, run_async(run()))


@draft_cli.command("adopt")
def draft_adopt(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    text: str | None = None,
    file: Path | None = None,
    title: str | None = None,
    sources: Annotated[Path | None, typer.Option("--sources")] = None,
    fork: bool = False,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    body = (
        read_body(text, file)
        if text is not None or file is not None
        else (sys.stdin.read() or None)
        if not sys.stdin.isatty()
        else None
    )
    changes = (
        SourceChanges.model_validate_json(input_bytes(sources)) if sources is not None else None
    )
    if body is None and changes is not None:
        raise NousError(
            "input.override_required",
            "Source changes require an explicit complete Markdown override",
        )
    override = DraftOverride(markdown=body, source_changes=changes) if body is not None else None

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).records.adopt(
                ref,
                override=override,
                title=title,
                fork=fork,
                after=after_write(refresh, refresh_mode, background, inline, annotate),
            )

    emit_record(ctx, run_async(run()))


@journal_cli.command("revise")
@note_cli.command("revise")
def propose_edit(
    ctx: typer.Context,
    ref: str,
    instruction: str = "",
    namespace: str | None = None,
    blocks: str | None = None,
    scopes: Annotated[Path | None, typer.Option("--scopes")] = None,
    feedback: Annotated[list[str] | None, typer.Option("--feedback")] = None,
    feedback_mappings: Annotated[Path | None, typer.Option("--feedback-mappings")] = None,
    intent: Literal["edit", "correction"] = "edit",
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    save: bool = False,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    selected = (
        TypeAdapter(tuple[BlockRef | ContentSelection, ...]).validate_json(input_bytes(scopes))
        if scopes is not None
        else None
    )
    mappings = (
        TypeAdapter(dict[str, str]).validate_json(input_bytes(feedback_mappings))
        if feedback_mappings
        else {}
    )

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            services = await runtime.cognition(namespace)
            return await services.writing.propose_edit(
                ref,
                instruction,
                scopes=selected,
                blocks=tuple(blocks.split(",")) if blocks else (),
                sources=source_requests(source, current_basis, inspiration),
                feedback_refs=feedback or (),
                feedback_mappings=mappings,
                intent=intent,
                adoption="save" if save else "draft",
                after=after_write(refresh, refresh_mode, background, inline, annotate),
            )

    emit_outcome(ctx, run_async(run()))


@journal_cli.command("rewrite")
def rewrite(
    ctx: typer.Context,
    ref: str,
    instruction: Annotated[str, typer.Option("--instruction")],
    intent: Literal["edit", "correction"] = "edit",
    namespace: str | None = None,
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    save: bool = False,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).writing.rewrite(
                ref,
                instruction,
                intent=intent,
                sources=source_requests(source, current_basis, inspiration),
                adoption="save" if save else "draft",
                after=after_write(refresh, refresh_mode, background, inline, annotate),
            )

    emit_outcome(ctx, run_async(run()))


@journal_cli.command("reflect")
def reflect(
    ctx: typer.Context,
    journals: list[str],
    topic: Annotated[str, typer.Option("--topic")],
    namespace: str | None = None,
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    author: str | None = None,
    viewpoint: str | None = None,
    save: bool = False,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).writing.reflect(
                journals,
                topic,
                sources=source_requests(source, current_basis, inspiration),
                author=author,
                viewpoint=viewpoint,
                adoption="save" if save else "draft",
                after=after_write(refresh, refresh_mode, background, inline, annotate),
            )

    emit_outcome(ctx, run_async(run()))
