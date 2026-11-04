"""Current cognitive retrieval through the shared application owner."""

from typing import Annotated, cast

import typer

from nous.cognition.retrieval import CognitiveQuery, Mode
from nous.runtime.bootstrap import open_instance

from . import cli, emit, options, run_async
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
    explain: bool = False,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.cognitive_retrieval.query(
                CognitiveQuery(
                    text=text,
                    namespace=namespace,
                    mode=mode,
                    snapshot=snapshot,
                    top_k=top_k,
                    include_review=True if include_review else None,
                    explain=explain,
                )
            )

    result = run_async(run())
    if options(ctx).json_mode:
        emit(ctx, result)
        return
    hits = cast(list[dict[str, object]], result["hits"])
    if not hits:
        typer.echo("没有找到匹配的认识 / No matching knowledge")
    for item in hits:
        typer.echo(f"{item['title']} — {item['revision']}\n{item['snippet']}\n")
        sources = cast(list[dict[str, object]], item["source_summary"])
        if sources:
            typer.echo(
                "来源 / Sources: "
                + ", ".join(
                    f"{source['key']} {source['name'] or ''} ({source['revision']})"
                    for source in sources
                )
            )
        if item["needs_review"]:
            typer.echo("待复核 / Needs review")
        if explain:
            typer.echo(f"  {item['channels']}  {item['ranking_score']}")
    if result["needs_review_count"]:
        typer.echo(f"待复核 / Needs review: {result['needs_review_count']}")


@journal_cli.command("refresh")
def refresh(
    ctx: typer.Context,
    namespace: str | None = None,
    mode: Annotated[Mode | None, typer.Option("--mode")] = None,
    include_review: bool = False,
) -> None:
    async def run() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.cognitive_retrieval.refresh(
                namespace, mode=mode, include_review=True if include_review else None
            )

    result = run_async(run())
    if options(ctx).json_mode:
        emit(ctx, result)
    else:
        typer.echo("检索已更新 / Retrieval updated")
