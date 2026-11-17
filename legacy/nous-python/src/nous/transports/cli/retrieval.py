"""Query and refresh the current cognitive view through typed operations."""

import json
from pathlib import Path
from typing import Annotated

import typer

from nous.application.inputs import InputProvider
from nous.cognition.retrieval.contracts import CognitiveQuery, Mode, RefreshRetrieval
from nous.cognition.retrieval.operations import QUERY, REFRESH
from nous.content.contracts import ExternalInput
from nous.presentation import translate
from nous.wave.contracts import CandidateFilter, ContextFragment, QueryCue

from .cognition import journal_cli
from .context import application_call, emit, language_for, options
from .groups import cli
from .input_files import json_binding


@cli.command("query")
def query(
    ctx: typer.Context,
    text: str,
    namespace: str | None = None,
    mode: Annotated[Mode | None, typer.Option("--mode")] = None,
    snapshot: str | None = None,
    top_k: Annotated[int | None, typer.Option("--top-k", min=1)] = None,
    include_review: bool = False,
    include_exploration: Annotated[
        bool | None, typer.Option("--include-exploration/--exclude-exploration")
    ] = None,
    context: Annotated[Path | None, typer.Option("--context", exists=True, dir_okay=False)] = None,
    core: Annotated[list[str] | None, typer.Option("--core")] = None,
    cue: Annotated[list[str] | None, typer.Option("--cue")] = None,
    filters: Annotated[Path | None, typer.Option("--filter", exists=True, dir_okay=False)] = None,
    purpose: str | None = None,
    explain: bool = False,
) -> None:
    inputs: dict[str, InputProvider] = {}
    context_value: tuple[ContextFragment, ...] | ExternalInput = ()
    filter_value: CandidateFilter | ExternalInput | None = None
    if context:
        context_value, extra = json_binding(context, key="context")
        inputs.update(extra)
    if filters:
        filter_value, extra = json_binding(filters, key="filters")
        inputs.update(extra)
    result = application_call(
        ctx,
        QUERY,
        CognitiveQuery(
            text=text,
            namespace=namespace,
            mode=mode,
            snapshot=snapshot,
            top_k=top_k,
            include_review=True if include_review else None,
            include_exploration=include_exploration,
            context=context_value,
            core_concepts=tuple(core or ()),
            cues=tuple(QueryCue(label=value, description=value) for value in cue or ()),
            filters=filter_value,
            purpose=purpose,
            explain=explain,
        ),
        inputs=inputs,
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
        return
    language = language_for(options(ctx))
    if not result.groups:
        typer.echo(translate("No matching knowledge", language))
    for group in result.groups:
        revision = group.context.revision_ref
        title = group.title or translate(
            "Untitled journal" if revision.record_ref.kind == "journal" else "Untitled note",
            language,
        )
        typer.echo(f"{group.rank}. {title} — {revision.identifier}")
        for passage in group.passages:
            if passage.delivery.reason == "title_preview":
                typer.echo("标题命中 / Title match")
            typer.echo(passage.delivery.text)
        if group.context.sources:
            sources = [
                f"{source.key} {source.name or ''} ({source.owner_revision.identifier if source.owner_revision else ', '.join(part.artifact.identifier for part in source.selections)})"
                for source in group.context.sources
            ]
            typer.echo(translate("Sources", language) + ": " + ", ".join(sources))
        if group.context.needs_review:
            typer.echo(translate("Needs review", language))
        if explain:
            typer.echo(json.dumps(group.model_dump(mode="json"), ensure_ascii=False, indent=2))
        typer.echo()
    if result.needs_review_count:
        typer.echo(translate("Needs review", language) + f": {result.needs_review_count}")


@journal_cli.command("refresh")
def refresh(
    ctx: typer.Context,
    namespace: str | None = None,
    mode: Annotated[Mode | None, typer.Option("--mode")] = None,
    include_review: bool = False,
) -> None:
    result = application_call(
        ctx,
        REFRESH,
        RefreshRetrieval(
            namespace=namespace,
            mode=mode,
            include_review=True if include_review else None,
        ),
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
    else:
        typer.echo(
            f"{translate('Retrieval updated', language_for(options(ctx)))}: {result.snapshot.identifier}"
        )
