"""Episode management uses the same typed SDK as HTTP and MCP callers."""

from pathlib import Path

import typer
from nous_wave.contracts import CognitiveRef
from nous_wave.contracts.episodes import EpisodeQuery, ProcessEpisodeSources, RebuildEpisodes

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Episode membership, summaries, sources and durable rebuilds.")
    cli.add_typer(group, name="episode")

    @group.command("list")
    def query(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        query: str = "",
        source: str | None = None,
        limit: int = 20,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.query_episodes(
                namespace_id, subject_id, EpisodeQuery(query=query, source=source, limit=limit)
            ),
        )

    @group.command("get")
    def get(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        episode_id: str,
        revision: int,
        paragraph_limit: int = 100,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.episode(
                namespace_id,
                subject_id,
                CognitiveRef(kind="episode", id=episode_id, revision=revision),
                paragraph_limit=paragraph_limit,
            ),
        )

    @group.command("rebuild")
    def rebuild(ctx: typer.Context, namespace_id: str, subject_id: str, request: Path) -> None:
        command = read_command(request, RebuildEpisodes)
        invoke(ctx, lambda api: api.rebuild_episodes(namespace_id, subject_id, command))

    @group.command("history")
    def history(
        ctx: typer.Context, namespace_id: str, subject_id: str, episode_id: str, limit: int = 100
    ) -> None:
        invoke(
            ctx, lambda api: api.episode_history(namespace_id, subject_id, episode_id, limit=limit)
        )

    @group.command("status")
    def status(ctx: typer.Context, namespace_id: str, subject_id: str, limit: int = 20) -> None:
        invoke(ctx, lambda api: api.episode_status(namespace_id, subject_id, limit=limit))

    @group.command("process")
    def process(
        ctx: typer.Context, namespace_id: str, subject_id: str, limit: int = 20, max_retry: int = 3
    ) -> None:
        invoke(
            ctx,
            lambda api: api.process_episode_sources(
                namespace_id, subject_id, ProcessEpisodeSources(limit=limit, max_retry=max_retry)
            ),
        )
