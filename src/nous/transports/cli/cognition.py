"""User-facing written record commands call the shared cognitive owners."""

import json
import sys
from pathlib import Path
from typing import Annotated, Literal

import typer
from pydantic import TypeAdapter

from nous.cognition.contracts import NamespaceRef, RecordView
from nous.cognition.retrieval.contracts import Mode
from nous.cognition.writing.contracts import (
    AfterWrite,
    CommittedRecord,
    RecordOutcome,
    SourceChanges,
    SourceRequest,
)
from nous.common.errors import NousError
from nous.content.edits import TextChange
from nous.presentation import translate
from nous.runtime.bootstrap import open_instance

from . import cli, emit_content_view, language_for, options, run_async
from .input_files import input_bytes

note_cli = typer.Typer(help="Notes", no_args_is_help=True)
journal_cli = typer.Typer(help="Journals", no_args_is_help=True)
namespace_cli = typer.Typer(help="Namespaces", no_args_is_help=True)
cli.add_typer(note_cli, name="note")
cli.add_typer(journal_cli, name="journal")
cli.add_typer(namespace_cli, name="namespace")


def read_body(text: str | None, file: Path | None) -> str:
    stdin = not sys.stdin.isatty()
    if text is not None and file is not None:
        raise NousError("input.conflicting_body", "Choose one of --text, --file or stdin")
    if stdin and (text is not None or file is not None) and sys.stdin.read():
        raise NousError("input.conflicting_body", "Choose one of --text, --file or stdin")
    if text is not None:
        return text
    if file is not None:
        return input_bytes(file).decode("utf-8")
    if stdin:
        return sys.stdin.read()
    raise NousError("input.body_required", "Provide --text, --file or stdin")


def emit_record(ctx: typer.Context, result: RecordView | CommittedRecord | RecordOutcome) -> None:
    language = language_for(options(ctx))
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    else:
        record = result.record if isinstance(result, RecordOutcome) else result
        title = record.title or translate(
            "Untitled note" if record.ref.kind == "note" else "Untitled journal", language
        )
        typer.echo(f"{title} — {record.revision_ref.identifier}")
        if isinstance(result, RecordView):
            typer.echo(result.body)
            if result.current_revision != result.revision_ref:
                typer.echo(
                    f"{translate('Current revision', language)}: {result.current_revision.identifier}"
                )
            for key, views in result.source_views.items():
                typer.echo(f"\n[{key}]")
                for view in views:
                    emit_content_view(view)
        elif isinstance(result, RecordOutcome):
            typer.echo(translate("Saved", language))
            for name, stage in (("Annotation", result.annotation), ("Retrieval", result.retrieval)):
                if stage.state != "not_requested":
                    typer.echo(f"{translate(name, language)}: {translate(stage.state, language)}")
                if stage.job:
                    typer.echo(f"Job: {stage.job.workflow_id}")
                if stage.issue:
                    typer.echo(f"{stage.issue.code}: {stage.issue.message}")
            if result.status == "partial":
                typer.echo(f"{translate('Operation', language)}: {result.operation.operation_id}")
    if isinstance(result, RecordOutcome) and result.status == "partial":
        raise typer.Exit(3)


def after_write(
    refresh: bool,
    mode: Mode | None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> AfterWrite:
    if background and inline:
        raise NousError("input.invalid", "Choose --background or --inline")
    return AfterWrite(
        annotation="run" if annotate else "default",
        retrieval="refresh" if refresh or mode is not None else "defer",
        retrieval_mode=mode,
        execution="background" if background else "inline" if inline else "default",
    )


def source_requests(
    source: list[str] | None, current_basis: list[str] | None, inspiration: list[str] | None
) -> tuple[SourceRequest, ...]:
    return (
        tuple(SourceRequest(target=target, usage="reference") for target in source or ())
        + tuple(
            SourceRequest(target=target, usage="current_basis") for target in current_basis or ()
        )
        + tuple(SourceRequest(target=target, usage="inspiration") for target in inspiration or ())
    )


async def _create(
    ctx: typer.Context,
    namespace: str | None,
    body: str,
    title: str | None,
    kind: str,
    remember: bool | None = None,
    purpose: str = "record",
    source: list[str] | None = None,
    current_basis: list[str] | None = None,
    inspiration: list[str] | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> RecordOutcome:
    async with open_instance(options(ctx).locator) as runtime:
        records = (await runtime.cognition(namespace)).records
        requests = source_requests(source, current_basis, inspiration)
        if kind == "note":
            return await records.add_note(
                body,
                title=title,
                remember=remember,
                source_requests=requests,
                after=after_write(refresh, refresh_mode, background, inline, annotate),
            )
        return await records.create_journal(
            body,
            title=title,
            purpose=purpose,
            source_requests=requests,
            after=after_write(refresh, refresh_mode, background, inline, annotate),
        )


@note_cli.command("add")
def note_add(
    ctx: typer.Context,
    text: str | None = None,
    file: Path | None = None,
    title: str | None = None,
    remember: Annotated[bool | None, typer.Option("--remember/--no-remember")] = None,
    namespace: str | None = None,
    source: Annotated[
        list[str] | None,
        typer.Option(
            "--source", help='Fixed source record, e.g. "note(...)"; repeated inputs use S1, S2...'
        ),
    ] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    emit_record(
        ctx,
        run_async(
            _create(
                ctx,
                namespace,
                read_body(text, file),
                title,
                "note",
                remember,
                source=source,
                current_basis=current_basis,
                inspiration=inspiration,
                refresh=refresh,
                refresh_mode=refresh_mode,
                background=background,
                inline=inline,
                annotate=annotate,
            )
        ),
    )


@journal_cli.command("create")
def journal_create(
    ctx: typer.Context,
    text: str | None = None,
    file: Path | None = None,
    title: str | None = None,
    purpose: str = "record",
    namespace: str | None = None,
    source: Annotated[
        list[str] | None,
        typer.Option(
            "--source", help='Fixed source record, e.g. "note(...)"; repeated inputs use S1, S2...'
        ),
    ] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    emit_record(
        ctx,
        run_async(
            _create(
                ctx,
                namespace,
                read_body(text, file),
                title,
                "journal",
                purpose=purpose,
                source=source,
                current_basis=current_basis,
                inspiration=inspiration,
                refresh=refresh,
                refresh_mode=refresh_mode,
                background=background,
                inline=inline,
                annotate=annotate,
            )
        ),
    )


@cli.command("read")
def read_record(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    sources: bool = False,
    blocks: str | None = None,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).records.read(
                ref,
                sources=sources,
                blocks=tuple(blocks.split(",")) if blocks else (),
            )

    emit_record(ctx, run_async(run()))


@journal_cli.command("replace")
@note_cli.command("replace")
def revise_record(
    ctx: typer.Context,
    ref: str,
    text: str | None = None,
    file: Path | None = None,
    expected: str | None = None,
    title: str | None = None,
    reason: str | None = None,
    intent: Literal["edit", "correction"] = "edit",
    feedback: Annotated[list[str] | None, typer.Option("--feedback")] = None,
    feedback_mappings: Annotated[Path | None, typer.Option("--feedback-mappings")] = None,
    namespace: str | None = None,
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    clear_sources: bool = False,
    source_changes: Annotated[Path | None, typer.Option("--sources")] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    if (source or current_basis or inspiration) and clear_sources:
        raise NousError("input.conflicting_sources", "Choose --source or --clear-sources")
    if source_changes is not None and clear_sources:
        raise NousError("input.conflicting_sources", "Choose source changes or --clear-sources")
    changes = (
        SourceChanges.model_validate_json(input_bytes(source_changes))
        if source_changes
        else SourceChanges(preserve=False)
        if clear_sources
        else None
    )
    body = read_body(text, file)
    mappings = (
        TypeAdapter(dict[str, str]).validate_json(input_bytes(feedback_mappings))
        if feedback_mappings
        else {}
    )

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            records = (await runtime.cognition(namespace)).records
            bindings = (
                source_requests(source, current_basis, inspiration)
                if source is not None or current_basis is not None or inspiration is not None
                else (() if clear_sources else None)
            )
            return await records.replace(
                ref,
                body,
                expected=expected,
                title=title,
                reason=reason,
                intent=intent,
                feedback_refs=feedback or (),
                feedback_mappings=mappings,
                source_requests=bindings,
                sources=changes,
                after=after_write(refresh, refresh_mode, background, inline, annotate),
            )

    emit_record(ctx, run_async(run()))


@journal_cli.command("patch")
@note_cli.command("patch")
def patch_record(
    ctx: typer.Context,
    ref: str,
    changes: Annotated[Path, typer.Option("--changes")],
    source_changes: Annotated[Path | None, typer.Option("--sources")] = None,
    namespace: str | None = None,
    reason: str | None = None,
    intent: Literal["edit", "correction"] = "edit",
    feedback: Annotated[list[str] | None, typer.Option("--feedback")] = None,
    feedback_mappings: Annotated[Path | None, typer.Option("--feedback-mappings")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
):
    parsed = TypeAdapter(list[TextChange]).validate_json(input_bytes(changes))
    mappings = (
        TypeAdapter(dict[str, str]).validate_json(input_bytes(feedback_mappings))
        if feedback_mappings
        else {}
    )
    bindings = (
        SourceChanges.model_validate_json(input_bytes(source_changes)) if source_changes else None
    )

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).records.apply(
                ref,
                parsed,
                sources=bindings,
                reason=reason,
                intent=intent,
                feedback_refs=feedback or (),
                feedback_mappings=mappings,
                after=after_write(refresh, refresh_mode, background, inline, annotate),
            )

    emit_record(ctx, run_async(run()))


@journal_cli.command("diff")
@note_cli.command("diff")
def record_diff(ctx: typer.Context, left: str, right: str, namespace: str | None = None):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).records.diff(left, right)

    result = run_async(run())
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.unified)


@journal_cli.command("edit")
@note_cli.command("edit")
def edit_record(
    ctx: typer.Context,
    ref: Annotated[str | None, typer.Argument()] = None,
    namespace: str | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
    resume: Annotated[Path | None, typer.Option("--resume")] = None,
):
    from difflib import unified_diff

    import click

    from .editing import EditSession

    selected = options(ctx)
    language = language_for(selected)
    if resume is not None:
        kind = ctx.parent.info_name if ctx.parent else ""
        session = EditSession.resume(resume, selected.locator, kind or "")
        if (refresh or refresh_mode or background or inline or annotate) and after_write(
            refresh, refresh_mode, background, inline, annotate
        ) != session.after:
            raise NousError(
                "input.edit_session_mismatch",
                "The edit recovery request has different maintenance options.",
            )
    else:
        if ref is None:
            raise NousError("input.body_required", "Provide a record or --resume PATH.")

        async def load():
            async with open_instance(selected.locator) as runtime:
                original = await (await runtime.cognition(namespace)).records.read(ref)
                return EditSession.create(
                    runtime.paths,
                    original,
                    after_write(refresh, refresh_mode, background, inline, annotate),
                )

        session = run_async(load())

    async def save():
        async with open_instance(selected.locator) as runtime:
            if runtime.descriptor.instance_id != session.target.instance_id:
                raise NousError(
                    "input.edit_session_mismatch", "The edit session belongs to another instance."
                )
            records = (
                await runtime.cognition(
                    NamespaceRef(
                        instance_id=session.target.instance_id, id=session.target.namespace_id
                    )
                )
            ).records
            if ref is not None:
                requested = await records.read(ref)
                if requested.ref != session.target:
                    raise NousError(
                        "input.edit_session_mismatch", "The edit session belongs to another target."
                    )
            try:
                return await records.replace(
                    session.target,
                    body,
                    expected=session.expected,
                    write_key=session.id,
                    after=session.after,
                )
            except NousError as error:
                if error.code == "cognition.revision_conflict":
                    current = await records.read(session.target)
                    error.details.update(
                        current=current.revision_ref.identifier,
                        expected=session.expected.identifier,
                        diff="".join(
                            unified_diff(
                                current.body.splitlines(keepends=True),
                                body.splitlines(keepends=True),
                                fromfile=current.revision_ref.identifier,
                                tofile=str(session.file),
                            )
                        ),
                    )
                raise

    try:
        if resume is None:
            click.edit(filename=str(session.file))
        body = session.read()
        if session.unchanged(body):
            session.clear()
            return
        result = run_async(save())
        try:
            emit_record(ctx, result)
        except typer.Exit as exit:
            if exit.exit_code == 3:  # A partial outcome still confirms the written revision.
                session.clear()
            raise
        else:
            session.clear()
    except BaseException as error:
        if session.descriptor.exists():
            if isinstance(error, NousError):
                error.details.update(
                    edited_file=str(session.file), edit_session=str(session.descriptor)
                )
            typer.echo(f"{translate('Edit recovery file', language)}: {session.file}", err=True)
            typer.echo(
                f"{translate('Edit recovery session', language)}: {session.descriptor}", err=True
            )
        raise


@journal_cli.command("history")
@note_cli.command("history")
def history(ctx: typer.Context, ref: str, namespace: str | None = None):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).records.history(ref)

    result = run_async(run())
    if options(ctx).json_mode:
        typer.echo(
            json.dumps(
                {"revisions": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
            )
        )
    else:
        for item in result:
            typer.echo(
                f"{item.revision_ref.identifier}  {item.title or translate('Untitled note' if item.revision_ref.record_ref.kind == 'note' else 'Untitled journal', language_for(options(ctx)))}  {item.recorded_at.isoformat()}"
            )


async def _membership(
    ctx: typer.Context,
    ref: str,
    namespace: str | None,
    operation: str,
    expected: str | None = None,
    enabled: bool | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
) -> RecordOutcome:
    after = after_write(refresh, refresh_mode, background, inline)
    async with open_instance(options(ctx).locator) as runtime:
        if operation == "recall":
            return await (await runtime.cognition(namespace)).records.set_recall(
                ref, bool(enabled), expected=expected, after=after
            )
        return await getattr((await runtime.cognition(namespace)).records, operation)(
            ref, expected=expected, after=after
        )


@journal_cli.command("archive")
@note_cli.command("archive")
def archive(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    expected: str | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
):
    emit_record(
        ctx,
        run_async(
            _membership(
                ctx,
                ref,
                namespace,
                "archive",
                expected,
                refresh=refresh,
                refresh_mode=refresh_mode,
                background=background,
                inline=inline,
            )
        ),
    )


@journal_cli.command("restore")
@note_cli.command("restore")
def restore(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    expected: str | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
):
    emit_record(
        ctx,
        run_async(
            _membership(
                ctx,
                ref,
                namespace,
                "restore",
                expected,
                refresh=refresh,
                refresh_mode=refresh_mode,
                background=background,
                inline=inline,
            )
        ),
    )


@note_cli.command("remember")
def remember(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    expected: str | None = None,
    enabled: bool = True,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
):
    emit_record(
        ctx,
        run_async(
            _membership(
                ctx,
                ref,
                namespace,
                "recall",
                expected,
                enabled,
                refresh=refresh,
                refresh_mode=refresh_mode,
                background=background,
                inline=inline,
            )
        ),
    )


@journal_cli.command("list")
@note_cli.command("list")
def list_records(
    ctx: typer.Context,
    namespace: str | None = None,
    archived: bool = False,
    offset: int = 0,
    limit: int | None = None,
):
    kind = "note" if ctx.parent is not None and ctx.parent.info_name == "note" else "journal"

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).records.list(
                kind=kind, archived=archived, offset=offset, limit=limit
            )

    result = run_async(run())
    if options(ctx).json_mode:
        typer.echo(
            json.dumps(
                {"records": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
            )
        )
    else:
        for item in result:
            typer.echo(
                f"{item.revision_ref.identifier}  {item.title or translate('Untitled note' if item.revision_ref.record_ref.kind == 'note' else 'Untitled journal', language_for(options(ctx)))}"
            )


@namespace_cli.command("create")
def namespace_create(ctx: typer.Context, name: str):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.create(name)

    result = run_async(run())
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("list")
def namespace_list(ctx: typer.Context):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.list()

    result = run_async(run())
    typer.echo(
        json.dumps(
            {"namespaces": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
        )
        if options(ctx).json_mode
        else "\n".join(item.name for item in result)
    )


@namespace_cli.command("rename")
def namespace_rename(ctx: typer.Context, namespace: str, name: str):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.rename(namespace, name)

    result = run_async(run())
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("select")
def namespace_select(ctx: typer.Context, namespace: str):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.select(namespace)

    result = run_async(run())
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)
