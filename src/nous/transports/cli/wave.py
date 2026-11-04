"""Wave product actions; argument conversion and presentation only."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Annotated, Literal, cast

import typer
from pydantic import TypeAdapter

from nous.application.embedding import VectorRef
from nous.runtime.bootstrap import open_instance
from nous.wave.contracts import (
    CandidateFilter,
    CandidatePool,
    ContextFragment,
    QueryCue,
    ThinkingStage,
    WaveQuery,
)

from . import emit, options, run_async

wave_cli = typer.Typer(help="构建、检索和回读内容集合 / Wave retrieval", no_args_is_help=True)


def _context(path: Path | None) -> tuple[ContextFragment, ...]:
    return TypeAdapter(tuple[ContextFragment, ...]).validate_json(path.read_bytes()) if path else ()


def _hits(value: dict[str, object], explain: bool) -> None:
    hits = cast(list[dict[str, object]], value.get("hits", []))
    for rank, hit in enumerate(hits, 1):
        ranking = hit.get("ranking_score")
        quality = hit.get("quality_score")
        scores = f"ranking_score={ranking}"
        if quality is not None:
            scores += f"  quality_score={quality}"
        typer.echo(f"{rank}. {hit['candidate']}  {scores}  ({value.get('score_kind', '')})")
        typer.echo(str(hit.get("snippet", "")))
        content = cast(dict[str, object], hit.get("content", {}))
        artifact = cast(dict[str, object], content.get("artifact", {}))
        typer.echo(
            f"   {artifact.get('address', '')} {json.dumps(content.get('selection', {}), ensure_ascii=False)}"
        )
        if hit.get("concepts"):
            typer.echo("   Concepts: " + ", ".join(cast(list[str], hit["concepts"])))
        if hit.get("sources"):
            typer.echo("   Sources: " + json.dumps(hit["sources"], ensure_ascii=False))
        if hit.get("folded_refs"):
            typer.echo("   Related sources: " + json.dumps(hit["folded_refs"], ensure_ascii=False))
        if explain:
            details = {
                key: item
                for key, item in hit.items()
                if key not in {"snippet", "content", "candidate_ref"}
            }
            typer.echo(json.dumps(details, ensure_ascii=False, indent=2))
    if not hits:
        typer.echo("No matches / 没有匹配内容")
    if explain:
        typer.echo(
            json.dumps(
                {key: item for key, item in value.items() if key != "hits"},
                ensure_ascii=False,
                indent=2,
            )
        )


@wave_cli.command("build")
def build(
    ctx: typer.Context,
    manifest: Annotated[Path, typer.Option("--manifest", exists=True, dir_okay=False)],
    space: Annotated[str | None, typer.Option("--space")] = None,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.wave.build(manifest, space)

    emit(ctx, run_async(run()))


@wave_cli.command("inspect")
def inspect(
    ctx: typer.Context,
    snapshot: str,
    concept: Annotated[
        list[str] | None, typer.Option("--concept", help="概念 key；可重复 / Concept keys")
    ] = None,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            return (
                await runtime.wave.neighborhood(snapshot, concept)
                if concept
                else await runtime.wave.inspect(snapshot)
            )

    emit(ctx, run_async(run()))


@wave_cli.command("query")
def query(
    ctx: typer.Context,
    snapshot: str,
    text: Annotated[str, typer.Argument()] = "",
    mode: Annotated[
        Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"] | None,
        typer.Option("--mode"),
    ] = None,
    context: Annotated[Path | None, typer.Option("--context", exists=True, dir_okay=False)] = None,
    core: Annotated[
        list[str] | None, typer.Option("--core", help="核心概念 key；可重复 / Core concept keys")
    ] = None,
    cue: Annotated[
        list[str] | None, typer.Option("--cue", help="临时线索描述；可重复 / Cue descriptions")
    ] = None,
    thinking_stages: Annotated[
        Path | None, typer.Option("--thinking-stages", exists=True, dir_okay=False)
    ] = None,
    candidate_filter: Annotated[
        Path | None, typer.Option("--filter", exists=True, dir_okay=False)
    ] = None,
    vector_ref: Annotated[
        Path | None, typer.Option("--vector-ref", exists=True, dir_okay=False)
    ] = None,
    candidate_pool: Annotated[
        Path | None, typer.Option("--candidate-pool", exists=True, dir_okay=False)
    ] = None,
    top_k: Annotated[int | None, typer.Option("--top-k", min=1)] = None,
    explain: Annotated[bool, typer.Option("--explain")] = False,
) -> None:
    request = WaveQuery(
        snapshot=snapshot,
        text=text,
        vector=VectorRef.model_validate_json(vector_ref.read_bytes()) if vector_ref else None,
        candidate_pool=CandidatePool.model_validate_json(candidate_pool.read_bytes())
        if candidate_pool
        else None,
        mode=mode,
        context=_context(context),
        core_concepts=tuple(core or ()),
        cues=tuple(QueryCue(label=value, description=value) for value in cue or ()),
        thinking_stages=TypeAdapter(tuple[ThinkingStage, ...]).validate_json(
            thinking_stages.read_bytes()
        )
        if thinking_stages
        else (),
        candidate_filter=CandidateFilter.model_validate_json(candidate_filter.read_bytes())
        if candidate_filter
        else None,
        top_k=top_k,
        include_trace=explain,
    )

    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.wave.query(request)

    value = run_async(run())
    if options(ctx).json_mode:
        emit(ctx, value)
    else:
        _hits(value, explain)


@wave_cli.command("read")
def read(
    ctx: typer.Context,
    snapshot: str,
    candidate: str,
    sources: Annotated[bool, typer.Option("--sources")] = False,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.wave.read(snapshot, candidate, sources=sources)

    value = run_async(run())
    if options(ctx).json_mode:
        emit(ctx, value)
    else:
        typer.echo(str(value.get("text", "")))
        if sources:
            emit(ctx, {key: item for key, item in value.items() if key != "text"})


@wave_cli.command("segment")
def segment(
    ctx: typer.Context,
    snapshot: str,
    context: Annotated[Path, typer.Argument(exists=True, dir_okay=False)],
) -> None:
    fragments = _context(context)

    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.wave.segment(snapshot, fragments)

    emit(ctx, run_async(run()))
