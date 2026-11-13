"""Writing and draft commands using completed products and exact references."""

from pathlib import Path
from typing import Annotated, Literal

import typer

from nous.application.contracts import EventEnvelope, TextDelta
from nous.application.inputs import FileInput, InputProvider
from nous.cognition.writing.contracts import (
    AdoptDraft,
    DraftFromResult,
    EditDraftRequest,
    NewDraftRequest,
    ReadDraft,
    ReadRecord,
    ReflectDraftRequest,
    ReviseDraft,
    RewriteDraftRequest,
    SourceChoice,
    WritingOutcome,
)
from nous.cognition.writing.generation import (
    DRAFT_FROM_RESULT,
    READ_DRAFT,
    REVISE_DRAFT,
    WRITE_DRAFT,
)
from nous.cognition.writing.operations import ADOPT_DRAFT, READ_RECORD
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, ExternalInput
from nous.presentation import translate

from .cognition import after_write, emit_record, journal_cli, note_cli, source_requests
from .context import application_call, emit, emit_content_view, language_for, options
from .groups import cli
from .input_files import body_binding, json_binding

draft_cli = typer.Typer(help="Journal drafts", no_args_is_help=True)
journal_cli.add_typer(draft_cli, name="draft")


def emit_outcome(ctx: typer.Context, result: WritingOutcome) -> None:
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
        if result.record is not None and result.record.issues:
            raise typer.Exit(3)
        return
    typer.echo(f"Result: {result.product.identifier}")
    if result.draft is not None:
        draft = application_call(ctx, READ_DRAFT, ReadDraft(draft=result.draft))
        typer.echo(f"{translate('Draft', language_for(options(ctx)))}: {draft.ref.identifier}")
        typer.echo(draft.diff if draft.diff is not None else draft.body)
    elif result.status == "no_material":
        typer.echo(translate("No new material", language_for(options(ctx))))
    if result.record is not None:
        emit_record(ctx, result.record)


def _sources(
    source: list[str] | None,
    current_basis: list[str] | None,
    inspiration: list[str] | None,
    files: list[Path] | None = None,
) -> tuple[tuple[SourceChoice, ...], dict[str, InputProvider]]:
    choices = list(source_requests(source, current_basis, inspiration))
    inputs: dict[str, InputProvider] = {}
    for index, path in enumerate(files or (), 1):
        key = f"source_file_{index}"
        choices.append(SourceChoice(target=ExternalInput(key=key)))
        inputs[key] = FileInput(path, name=path.name, source=str(path))
    return tuple(choices), inputs


def _exact(ctx: typer.Context, ref: str, namespace: str | None):
    if "@" not in ref:
        raise NousError("input.expected_required", "Provide an exact revision with @N")
    return application_call(ctx, READ_RECORD, ReadRecord(target=ref, namespace=namespace))


@cli.command("journalize")
def journalize(
    ctx: typer.Context,
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    file: Annotated[list[Path] | None, typer.Option("--file")] = None,
    topic: str | None = None,
    purpose: str = "record",
    author: str | None = None,
    viewpoint: str | None = None,
    save: Annotated[bool | None, typer.Option("--save/--draft")] = None,
    namespace: str | None = None,
    stream: bool = False,
    refresh: bool = False,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
) -> None:
    choices, inputs = _sources(source, current_basis, inspiration, file)

    def observer(event: EventEnvelope) -> None:
        if isinstance(event.event, TextDelta):
            typer.echo(event.event.text, nl=False, err=True)

    try:
        result = application_call(
            ctx,
            WRITE_DRAFT,
            NewDraftRequest(
                namespace=namespace,
                sources=choices,
                topic=topic,
                purpose=purpose,
                author=author,
                viewpoint=viewpoint,
                adoption="save" if save else "draft",
                stream=stream,
                after=after_write(refresh, background, inline, annotate),
            ),
            inputs=inputs,
            observer=observer if stream else None,
        )
    finally:
        if stream:
            typer.echo(err=True)
    emit_outcome(ctx, result)


@draft_cli.command("read")
def draft_read(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    sources: bool = False,
    full_body: Annotated[bool, typer.Option("--body")] = False,
) -> None:
    result = application_call(
        ctx, READ_DRAFT, ReadDraft(draft=ref, namespace=namespace, sources=sources)
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
        return
    typer.echo(result.ref.identifier)
    typer.echo(
        result.body
        if full_body or result.diff is None
        else (result.diff or translate("No changes", language_for(options(ctx))))
    )
    if result.adopted_revision:
        typer.echo(
            f"{translate('Adopted', language_for(options(ctx)))}: {result.adopted_revision.identifier}"
        )
    for key, views in result.source_views.items():
        typer.echo(f"\n[{key}]")
        for view in views:
            emit_content_view(view)


@draft_cli.command("from-result")
def draft_from_result(ctx: typer.Context, result: str) -> None:
    emit_outcome(ctx, application_call(ctx, DRAFT_FROM_RESULT, DraftFromResult(result=result)))


@draft_cli.command("revise")
def draft_revise(
    ctx: typer.Context,
    ref: str,
    text: str | None = None,
    file: Path | None = None,
    sources: Annotated[Path | None, typer.Option("--sources")] = None,
    namespace: str | None = None,
) -> None:
    body, inputs = body_binding(text, file)
    changes = None
    if sources:
        changes, extra = json_binding(sources, key="source_changes")
        inputs.update(extra)
    result = application_call(
        ctx,
        REVISE_DRAFT,
        ReviseDraft(draft=ref, namespace=namespace, markdown=body, sources=changes),
        inputs=inputs,
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
    else:
        typer.echo(result.ref.identifier)


@draft_cli.command("adopt")
def draft_adopt(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    title: str | None = None,
    fork: bool = False,
    refresh: bool = False,
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> None:
    result = application_call(
        ctx,
        ADOPT_DRAFT,
        AdoptDraft(
            draft=ref,
            namespace=namespace,
            title=title,
            fork=fork,
            after=after_write(refresh, background, inline, annotate),
        ),
    )
    emit_record(ctx, result)


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
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> None:
    if blocks and scopes:
        raise NousError("input.conflicting_scope", "Choose blocks or scopes")
    base = _exact(ctx, ref, namespace)
    inputs: dict[str, InputProvider] = {}
    targets: tuple[ContentSelection, ...] | ExternalInput = (base.body_selection,)
    if blocks:
        selected = application_call(
            ctx,
            READ_RECORD,
            ReadRecord(
                target=base.revision_ref, namespace=namespace, blocks=tuple(blocks.split(","))
            ),
        )
        targets = selected.selection.selections
    elif scopes:
        targets, extra = json_binding(scopes, key="targets")
        inputs.update(extra)
    mappings: dict[str, str] | ExternalInput = {}
    if feedback_mappings:
        mappings, extra = json_binding(feedback_mappings, key="feedback_mappings")
        inputs.update(extra)
    result = application_call(
        ctx,
        WRITE_DRAFT,
        EditDraftRequest(
            namespace=namespace,
            base=base.revision_ref,
            targets=targets,
            instruction=instruction,
            sources=source_requests(source, current_basis, inspiration),
            feedback=tuple(feedback or ()),
            feedback_mappings=mappings,
            change_kind=intent,
            adoption="save" if save else "draft",
            after=after_write(refresh, background, inline, annotate),
        ),
        inputs=inputs,
    )
    emit_outcome(ctx, result)


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
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> None:
    base = _exact(ctx, ref, namespace)
    result = application_call(
        ctx,
        WRITE_DRAFT,
        RewriteDraftRequest(
            namespace=namespace,
            base=base.revision_ref,
            instruction=instruction,
            change_kind=intent,
            sources=source_requests(source, current_basis, inspiration),
            adoption="save" if save else "draft",
            after=after_write(refresh, background, inline, annotate),
        ),
    )
    emit_outcome(ctx, result)


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
    background: bool = False,
    inline: bool = False,
    annotate: bool = False,
) -> None:
    exact = tuple(_exact(ctx, item, namespace).revision_ref for item in journals)
    result = application_call(
        ctx,
        WRITE_DRAFT,
        ReflectDraftRequest(
            journals=exact,
            topic=topic,
            namespace=namespace,
            sources=source_requests(source, current_basis, inspiration),
            author=author,
            viewpoint=viewpoint,
            adoption="save" if save else "draft",
            after=after_write(refresh, background, inline, annotate),
        ),
    )
    emit_outcome(ctx, result)
