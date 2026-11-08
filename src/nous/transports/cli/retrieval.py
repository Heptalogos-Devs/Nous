"""Query exact grouped cognitive results through the shared application owner."""

import json
from pathlib import Path
from typing import Annotated, cast

import typer

from nous.application.inputs import InputParameter
from nous.cognition.retrieval.contracts import Mode, QueryResult
from nous.presentation import translate
from nous.wave.contracts import QueryCue

from . import application_action, cli, emit, language_for, options
from .cognition import journal_cli
from .input_files import json_parameter


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
    request = {
        "text": text,
        "mode": mode,
        "snapshot": snapshot,
        "top_k": top_k,
        "include_review": True if include_review else None,
        "include_exploration": include_exploration,
        "context": (),
        "core_concepts": tuple(core or ()),
        "cues": tuple(QueryCue(label=value, description=value) for value in cue or ()),
        "filters": None,
        "purpose": purpose,
        "explain": explain,
    }
    parameters: list[InputParameter] = []
    if context:
        parameters.append(json_parameter("request.context", context))
    if filters:
        parameters.append(json_parameter("request.filters", filters))
    result = cast(
        QueryResult,
        application_action(
            ctx, "retrieval.query", scope=namespace, request=request, input_parameters=parameters
        ),
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
        return
    language = language_for(options(ctx))
    if not result.groups:
        typer.echo(translate("No matching knowledge", language))
    for group in result.groups:
        title = group.title or translate(
            "Untitled journal"
            if group.revision_ref.record_ref.kind == "journal"
            else "Untitled note",
            language,
        )
        typer.echo(f"{group.group_rank}. {title} — {group.revision_ref.identifier}")
        for passage in group.passages:
            typer.echo(passage.snippet)
        if group.sources:
            sources = [
                f"{source.key} {source.name or ''} ({source.owner_revision.identifier if source.owner_revision else ', '.join(part.artifact.identifier for part in source.selections)})"
                for source in group.sources
            ]
            typer.echo(translate("Sources", language) + ": " + ", ".join(sources))
        if group.needs_review:
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
    result = cast(
        dict[str, object],
        application_action(
            ctx,
            "retrieval.refresh",
            scope=namespace,
            mode=mode,
            include_review=True if include_review else None,
        ),
    )
    if options(ctx).json_mode:
        emit(ctx, result)
    else:
        typer.echo(translate("Retrieval updated", language_for(options(ctx))))
