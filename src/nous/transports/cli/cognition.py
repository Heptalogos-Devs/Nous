"""User-facing written record commands call the shared cognitive owners."""

import json
from pathlib import Path
from typing import Annotated, Literal, cast

import typer

from nous.application.inputs import InputParameter
from nous.cognition.contracts import Namespace, RecordView, RevisionSummary
from nous.cognition.retrieval.contracts import Mode
from nous.cognition.writing.contracts import (
    AfterWrite,
    CommittedRecord,
    RecordOutcome,
    SourceChanges,
    SourceRequest,
)
from nous.common.errors import NousError
from nous.presentation import translate

from . import application_action, cli, emit_content_view, language_for, options
from .input_files import body_parameter, json_parameter

note_cli = typer.Typer(help="Notes", no_args_is_help=True)
journal_cli = typer.Typer(help="Journals", no_args_is_help=True)
namespace_cli = typer.Typer(help="Namespaces", no_args_is_help=True)
cli.add_typer(note_cli, name="note")
cli.add_typer(journal_cli, name="journal")
cli.add_typer(namespace_cli, name="namespace")


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


def _create(
    ctx: typer.Context,
    namespace: str | None,
    body: InputParameter,
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
    requests = source_requests(source, current_basis, inspiration)
    after = after_write(refresh, refresh_mode, background, inline, annotate)
    if kind == "note":
        result = application_action(
            ctx,
            "records.add_note",
            scope=namespace,
            title=title,
            remember=remember,
            source_requests=requests,
            after=after,
            input_parameters=(body,),
        )
    else:
        result = application_action(
            ctx,
            "records.create_journal",
            scope=namespace,
            title=title,
            purpose=purpose,
            source_requests=requests,
            after=after,
            input_parameters=(body,),
        )
    return cast(RecordOutcome, result)


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
        _create(
            ctx,
            namespace,
            body_parameter("body", text, file),
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
        _create(
            ctx,
            namespace,
            body_parameter("body", text, file),
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
    result = application_action(
        ctx,
        "records.read",
        ref,
        scope=namespace,
        sources=sources,
        blocks=tuple(blocks.split(",")) if blocks else (),
    )
    emit_record(ctx, cast(RecordView, result))


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
    changes = SourceChanges(preserve=False) if clear_sources else None
    parameters = [body_parameter("markdown", text, file)]
    if source_changes:
        parameters.append(json_parameter("sources", source_changes))
    if feedback_mappings:
        parameters.append(json_parameter("feedback_mappings", feedback_mappings))

    bindings = (
        source_requests(source, current_basis, inspiration)
        if source is not None or current_basis is not None or inspiration is not None
        else (() if clear_sources else None)
    )
    result = application_action(
        ctx,
        "records.replace",
        ref,
        scope=namespace,
        expected=expected,
        title=title,
        reason=reason,
        intent=intent,
        feedback_refs=feedback or (),
        feedback_mappings={},
        source_requests=bindings,
        sources=changes,
        after=after_write(refresh, refresh_mode, background, inline, annotate),
        input_parameters=parameters,
    )
    emit_record(ctx, cast(RecordOutcome, result))


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
    parameters = [json_parameter("changes", changes)]
    if feedback_mappings:
        parameters.append(json_parameter("feedback_mappings", feedback_mappings))
    if source_changes:
        parameters.append(json_parameter("sources", source_changes))

    result = application_action(
        ctx,
        "records.apply",
        ref,
        scope=namespace,
        sources=None,
        reason=reason,
        intent=intent,
        feedback_refs=feedback or (),
        feedback_mappings={},
        after=after_write(refresh, refresh_mode, background, inline, annotate),
        input_parameters=parameters,
    )
    emit_record(ctx, cast(RecordOutcome, result))


@journal_cli.command("diff")
@note_cli.command("diff")
def record_diff(ctx: typer.Context, left: str, right: str, namespace: str | None = None):
    from nous.cognition.writing.contracts import RevisionDiff

    result = cast(
        RevisionDiff, application_action(ctx, "records.diff", left, right, scope=namespace)
    )
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

    from nous.application.editing import EditSession
    from nous.configuration import Configuration

    selected = options(ctx)
    language = language_for(selected)
    if resume is not None:
        kind = ctx.parent.info_name if ctx.parent else ""
        session = cast(
            EditSession, application_action(ctx, "records.resume_edit", resume, kind or "")
        )
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

        session = cast(
            EditSession,
            application_action(
                ctx,
                "records.begin_edit",
                ref,
                namespace,
                after_write(refresh, refresh_mode, background, inline, annotate),
            ),
        )

    try:
        if resume is None:
            click.edit(filename=str(session.file))
        body = cast(str, application_action(ctx, "records.read_edit", session.file))
        if session.unchanged(body):
            session.clear()
            return
        if (
            Configuration(selected.locator.config_file).snapshot.values.instance.id
            != session.target.instance_id
        ):
            raise NousError(
                "input.edit_session_mismatch", "The edit session belongs to another instance."
            )
        if ref is not None:
            requested = cast(
                RecordView,
                application_action(ctx, "records.read", ref, scope=session.target.namespace_id),
            )
            if requested.ref != session.target:
                raise NousError(
                    "input.edit_session_mismatch", "The edit session belongs to another target."
                )
        try:
            result = cast(
                RecordOutcome,
                application_action(
                    ctx,
                    "records.replace",
                    session.target,
                    body,
                    scope=session.target.namespace_id,
                    expected=session.expected,
                    write_key=session.id,
                    after=session.after,
                ),
            )
        except NousError as error:
            if error.code == "cognition.revision_conflict":
                current = cast(
                    RecordView,
                    application_action(
                        ctx, "records.read", session.target, scope=session.target.namespace_id
                    ),
                )
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
    result = cast(
        tuple[RevisionSummary, ...],
        application_action(ctx, "records.history", ref, scope=namespace),
    )
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


def _membership(
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
    if operation == "recall":
        result = application_action(
            ctx,
            "records.set_recall",
            ref,
            bool(enabled),
            scope=namespace,
            expected=expected,
            after=after,
        )
    else:
        result = application_action(
            ctx, f"records.{operation}", ref, scope=namespace, expected=expected, after=after
        )
    return cast(RecordOutcome, result)


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

    result = cast(
        tuple[CommittedRecord, ...],
        application_action(
            ctx,
            "records.list",
            scope=namespace,
            kind=kind,
            archived=archived,
            offset=offset,
            limit=limit,
        ),
    )
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
    result = cast(Namespace, application_action(ctx, "namespaces.create", name))
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("list")
def namespace_list(ctx: typer.Context):
    result = cast(tuple[Namespace, ...], application_action(ctx, "namespaces.list"))
    typer.echo(
        json.dumps(
            {"namespaces": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
        )
        if options(ctx).json_mode
        else "\n".join(item.name for item in result)
    )


@namespace_cli.command("rename")
def namespace_rename(ctx: typer.Context, namespace: str, name: str):
    result = cast(Namespace, application_action(ctx, "namespaces.rename", namespace, name))
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("select")
def namespace_select(ctx: typer.Context, namespace: str):
    result = cast(Namespace, application_action(ctx, "namespaces.select", namespace))
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)
