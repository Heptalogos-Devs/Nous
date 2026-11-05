"""Query exact grouped cognitive results through the shared application owner."""

import json
from pathlib import Path
from typing import Annotated

import typer
from pydantic import TypeAdapter

from nous.cognition.retrieval.contracts import CognitiveQuery, Mode, QueryResult
from nous.presentation import translate
from nous.runtime.bootstrap import open_instance
from nous.wave.contracts import CandidateFilter, ContextFragment, QueryCue

from . import cli, emit, language_for, options, run_async
from .cognition import journal_cli


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
    async def run() -> QueryResult:
        async with open_instance(options(ctx).locator) as runtime:
            cognition = await runtime.cognition(namespace)
            return await cognition.retrieval.query(
                CognitiveQuery(
                    text=text,
                    mode=mode,
                    snapshot=snapshot,
                    top_k=top_k,
                    include_review=True if include_review else None,
                    include_exploration=include_exploration,
                    context=TypeAdapter(tuple[ContextFragment, ...]).validate_json(
                        context.read_bytes()
                    )
                    if context
                    else (),
                    core_concepts=tuple(core or ()),
                    cues=tuple(QueryCue(label=value, description=value) for value in cue or ()),
                    filters=CandidateFilter.model_validate_json(filters.read_bytes())
                    if filters
                    else None,
                    purpose=purpose,
                    explain=explain,
                )
            )

    result = run_async(run())
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
        typer.echo(f"{group.group_rank}. {title} — {group.revision_ref.address}")
        for passage in group.passages:
            typer.echo(passage.snippet)
        if group.sources:
            sources = [
                f"{source.key} {source.name or ''} ({source.owner_revision.address if source.owner_revision else ', '.join(part.artifact.address for part in source.selections)})"
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
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            cognition = await runtime.cognition(namespace)
            return await cognition.retrieval.refresh(
                mode=mode, include_review=True if include_review else None
            )

    result = run_async(run())
    if options(ctx).json_mode:
        emit(ctx, result)
    else:
        typer.echo(translate("Retrieval updated", language_for(options(ctx))))
