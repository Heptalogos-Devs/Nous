"""Written record commands over the shared typed application operations."""

import json
from pathlib import Path
from typing import Annotated, Literal

import typer

from nous.cognition.contracts import (
    CreateNamespace,
    ListNamespaces,
    RecordView,
    RenameNamespace,
    SelectNamespace,
)
from nous.cognition.retrieval.contracts import Mode
from nous.cognition.scope import (
    CREATE_NAMESPACE,
    LIST_NAMESPACES,
    RENAME_NAMESPACE,
    SELECT_NAMESPACE,
)
from nous.cognition.writing.contracts import (
    AddNote,
    AfterWrite,
    ApplyEdits,
    CommittedRecord,
    CreateJournal,
    DiffRecords,
    ListRecords,
    ReadRecord,
    RecordMembership,
    RecordOutcome,
    ReplaceRecord,
    SetRecall,
    SourceChanges,
    SourceChoice,
)
from nous.cognition.writing.operations import (
    ADD_NOTE,
    APPLY_EDITS,
    ARCHIVE_RECORD,
    CREATE_JOURNAL,
    DIFF_RECORDS,
    HISTORY_RECORD,
    LIST_RECORDS,
    READ_RECORD,
    REPLACE_RECORD,
    RESTORE_RECORD,
    SET_RECALL,
)
from nous.common.errors import NousError
from nous.content.contracts import ExternalInput
from nous.presentation import translate

from .context import application_call, emit_content_view, language_for, options
from .groups import cli
from .input_files import body_binding, json_binding

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
        record = result.commit if isinstance(result, RecordOutcome) else result
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
            for source in result.source_details:
                typer.echo(f"\n[{source.binding.key}] {source.title or source.binding.name or ''}")
                for view in source.views:
                    emit_content_view(view)
        elif isinstance(result, RecordOutcome):
            typer.echo(translate("Saved", language))
            maintenance = result.maintenance
            if maintenance.execution == "started":
                typer.echo(
                    f"{translate('started', language)} · "
                    f"{translate('Operation', language)}: {maintenance.operation_ref.operation_id}"
                )
            elif maintenance.execution == "completed":
                for name, stage in (
                    ("Annotation", maintenance.annotation),
                    ("Retrieval", maintenance.refresh),
                ):
                    if stage.state != "not_requested":
                        typer.echo(
                            f"{translate(name, language)}: {translate(stage.state, language)}"
                        )
            for issue in result.issues:
                typer.echo(f"{issue.code}: {issue.message}", err=True)
    if isinstance(result, RecordOutcome) and result.issues:
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
        refresh="run" if refresh or mode is not None else "defer",
        mode=mode,
        execution="background" if background else "wait",
    )


def source_requests(
    source: list[str] | None, current_basis: list[str] | None, inspiration: list[str] | None
) -> tuple[SourceChoice, ...]:
    return (
        tuple(SourceChoice(target=target, usage="reference") for target in source or ())
        + tuple(
            SourceChoice(target=target, usage="current_basis") for target in current_basis or ()
        )
        + tuple(SourceChoice(target=target, usage="inspiration") for target in inspiration or ())
    )


def _create(
    ctx: typer.Context,
    namespace: str | None,
    text: str | None,
    file: Path | None,
    title: str | None,
    kind: Literal["note", "journal"],
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
    body, inputs = body_binding(text, file)
    sources = source_requests(source, current_basis, inspiration)
    policy = after_write(refresh, refresh_mode, background, inline, annotate)
    if kind == "note":
        return application_call(
            ctx,
            ADD_NOTE,
            AddNote(
                body=body,
                namespace=namespace,
                title=title,
                sources=sources,
                after=policy,
                remember=remember,
            ),
            inputs=inputs,
        )
    return application_call(
        ctx,
        CREATE_JOURNAL,
        CreateJournal(
            body=body,
            namespace=namespace,
            title=title,
            sources=sources,
            after=policy,
            purpose=purpose,
        ),
        inputs=inputs,
    )


@note_cli.command("add")
def note_add(
    ctx: typer.Context,
    text: str | None = None,
    file: Path | None = None,
    title: str | None = None,
    remember: Annotated[bool | None, typer.Option("--remember/--no-remember")] = None,
    namespace: str | None = None,
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> None:
    emit_record(
        ctx,
        _create(
            ctx,
            namespace,
            text,
            file,
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
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> None:
    emit_record(
        ctx,
        _create(
            ctx,
            namespace,
            text,
            file,
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
) -> None:
    result = application_call(
        ctx,
        READ_RECORD,
        ReadRecord(
            target=ref,
            namespace=namespace,
            sources=sources,
            blocks=tuple(blocks.split(",")) if blocks else (),
        ),
    )
    emit_record(ctx, result)


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
) -> None:
    if clear_sources and (source or current_basis or inspiration or source_changes):
        raise NousError("input.conflicting_sources", "Choose --clear-sources or source changes")
    body, inputs = body_binding(text, file)
    changes: SourceChanges | ExternalInput | None = (
        SourceChanges(preserve=False) if clear_sources else None
    )
    if source_changes:
        changes, extra = json_binding(source_changes, key="source_changes")
        inputs.update(extra)
    mappings: dict[str, str] | ExternalInput = {}
    if feedback_mappings:
        mappings, extra = json_binding(feedback_mappings, key="feedback_mappings")
        inputs.update(extra)
    result = application_call(
        ctx,
        REPLACE_RECORD,
        ReplaceRecord(
            target=ref,
            expected=expected,
            body=body,
            namespace=namespace,
            title=title,
            reason=reason,
            change_kind=intent,
            feedback_refs=tuple(feedback or ()),
            feedback_mappings=mappings,
            sources=source_requests(source, current_basis, inspiration),
            source_changes=changes,
            after=after_write(refresh, refresh_mode, background, inline, annotate),
        ),
        inputs=inputs,
    )
    emit_record(ctx, result)


@journal_cli.command("patch")
@note_cli.command("patch")
def patch_record(
    ctx: typer.Context,
    ref: str,
    changes: Annotated[Path, typer.Option("--changes")],
    source_changes: Annotated[Path | None, typer.Option("--sources")] = None,
    namespace: str | None = None,
    expected: str | None = None,
    reason: str | None = None,
    intent: Literal["edit", "correction"] = "edit",
    feedback: Annotated[list[str] | None, typer.Option("--feedback")] = None,
    feedback_mappings: Annotated[Path | None, typer.Option("--feedback-mappings")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> None:
    declared, inputs = json_binding(changes, key="changes")
    sources = None
    if source_changes:
        sources, extra = json_binding(source_changes, key="source_changes")
        inputs.update(extra)
    mappings: dict[str, str] | ExternalInput = {}
    if feedback_mappings:
        mappings, extra = json_binding(feedback_mappings, key="feedback_mappings")
        inputs.update(extra)
    result = application_call(
        ctx,
        APPLY_EDITS,
        ApplyEdits(
            target=ref,
            expected=expected,
            namespace=namespace,
            changes=declared,
            source_changes=sources,
            reason=reason,
            change_kind=intent,
            feedback_refs=tuple(feedback or ()),
            feedback_mappings=mappings,
            after=after_write(refresh, refresh_mode, background, inline, annotate),
        ),
        inputs=inputs,
    )
    emit_record(ctx, result)


@journal_cli.command("diff")
@note_cli.command("diff")
def record_diff(ctx: typer.Context, left: str, right: str, namespace: str | None = None) -> None:
    result = application_call(
        ctx, DIFF_RECORDS, DiffRecords(left=left, right=right, namespace=namespace)
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
) -> None:
    import click

    from nous.application.editing import EditSession
    from nous.configuration import Configuration
    from nous.runtime.instance import InstanceDescriptor
    from nous.runtime.paths import ResolvedPaths

    selected = options(ctx)
    settings = Configuration(selected.locator.config_file).snapshot.values
    policy = after_write(refresh, refresh_mode, background, inline, annotate)
    kind = ctx.parent.info_name if ctx.parent else ""
    if resume is not None:
        session = EditSession.resume(resume, resume.read_bytes(), settings.instance.id, kind or "")
        if (
            refresh or refresh_mode or background or inline or annotate
        ) and policy != session.after:
            raise NousError(
                "input.edit_session_mismatch", "The edit session has different maintenance options"
            )
    else:
        if ref is None:
            raise NousError("input.body_required", "Provide a record or --resume PATH")
        original = application_call(ctx, READ_RECORD, ReadRecord(target=ref, namespace=namespace))
        if original.ref.kind != kind:
            raise NousError("input.invalid_kind", "The record belongs to another command")
        descriptor = InstanceDescriptor(
            settings.instance.id, settings.instance.name, settings.instance.layout
        )
        paths = ResolvedPaths.resolve(selected.locator, descriptor, settings.paths)
        session = EditSession.create(paths, original, policy)
    try:
        if resume is None:
            click.edit(filename=str(session.file))
        if ref is not None:
            current = application_call(
                ctx, READ_RECORD, ReadRecord(target=ref, namespace=session.target.namespace_id)
            )
            if current.ref != session.target:
                raise NousError(
                    "input.edit_session_mismatch", "The edit session belongs to another record"
                )
        body, inputs = body_binding(None, session.file)
        result = application_call(
            ctx,
            REPLACE_RECORD,
            ReplaceRecord(
                target=session.target,
                expected=session.expected,
                namespace=session.target.namespace_id,
                body=body,
                write_key=session.id,
                after=session.after,
            ),
            inputs=inputs,
        )
        try:
            emit_record(ctx, result)
        except typer.Exit as error:
            if error.exit_code == 3:
                session.clear()
            raise
        session.clear()
    except BaseException as error:
        if session.descriptor.exists():
            if isinstance(error, NousError):
                error.details.update(
                    edited_file=str(session.file), edit_session=str(session.descriptor)
                )
            typer.echo(
                f"{translate('Edit recovery file', language_for(selected))}: {session.file}",
                err=True,
            )
            typer.echo(
                f"{translate('Edit recovery session', language_for(selected))}: {session.descriptor}",
                err=True,
            )
        raise


@journal_cli.command("history")
@note_cli.command("history")
def history(ctx: typer.Context, ref: str, namespace: str | None = None) -> None:
    result = application_call(ctx, HISTORY_RECORD, ReadRecord(target=ref, namespace=namespace))
    if options(ctx).json_mode:
        typer.echo(
            json.dumps(
                {"revisions": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
            )
        )
    else:
        for item in result:
            typer.echo(
                f"{item.revision_ref.identifier}  {item.title or ''}  {item.recorded_at.isoformat()}"
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
        return application_call(
            ctx,
            SET_RECALL,
            SetRecall(
                target=ref,
                namespace=namespace,
                expected=expected,
                enabled=bool(enabled),
                after=after,
            ),
        )
    request = RecordMembership(target=ref, namespace=namespace, expected=expected, after=after)
    return application_call(
        ctx, ARCHIVE_RECORD if operation == "archive" else RESTORE_RECORD, request
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
) -> None:
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
) -> None:
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
) -> None:
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
) -> None:
    kind = "note" if ctx.parent is not None and ctx.parent.info_name == "note" else "journal"
    result = application_call(
        ctx,
        LIST_RECORDS,
        ListRecords(namespace=namespace, kind=kind, archived=archived, offset=offset, limit=limit),
    )
    if options(ctx).json_mode:
        typer.echo(
            json.dumps(
                {"records": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
            )
        )
    else:
        for item in result:
            typer.echo(f"{item.revision_ref.identifier}  {item.title or ''}")


@namespace_cli.command("create")
def namespace_create(ctx: typer.Context, name: str) -> None:
    result = application_call(ctx, CREATE_NAMESPACE, CreateNamespace(name=name))
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("list")
def namespace_list(ctx: typer.Context) -> None:
    result = application_call(ctx, LIST_NAMESPACES, ListNamespaces())
    typer.echo(
        json.dumps(
            {"namespaces": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
        )
        if options(ctx).json_mode
        else "\n".join(item.name for item in result)
    )


@namespace_cli.command("rename")
def namespace_rename(ctx: typer.Context, namespace: str, name: str) -> None:
    result = application_call(
        ctx, RENAME_NAMESPACE, RenameNamespace(namespace=namespace, name=name)
    )
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("select")
def namespace_select(ctx: typer.Context, namespace: str) -> None:
    result = application_call(ctx, SELECT_NAMESPACE, SelectNamespace(namespace=namespace))
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)
