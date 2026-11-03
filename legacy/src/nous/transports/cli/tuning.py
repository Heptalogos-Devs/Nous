"""Retrieval tuning consumes a supplied CreateTuning request and publishes explicit profiles."""

from pathlib import Path

import typer
from nous_sdk.contracts.tuning import ApplyRetrievalProfile, CreateTuning, RollbackRetrievalProfile

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Tune retrieval with supplied relevance judgments.")
    cli.add_typer(group, name="tuning")

    @group.command("create")
    def create(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, CreateTuning)
        invoke(ctx, lambda api: api.create_tuning(namespace_id, command))

    @group.command("list")
    def tasks(
        ctx: typer.Context,
        namespace_id: str,
        limit: int = 50,
        cursor: str | None = None,
    ) -> None:
        invoke(ctx, lambda api: api.tuning_tasks(namespace_id, limit=limit, cursor=cursor))

    @group.command("get")
    def get(ctx: typer.Context, namespace_id: str, task_id: str) -> None:
        invoke(ctx, lambda api: api.tuning_task(namespace_id, task_id))

    @group.command("rounds")
    def rounds(
        ctx: typer.Context,
        namespace_id: str,
        task_id: str,
        limit: int = 100,
        after: int | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.tuning_rounds(namespace_id, task_id, limit=limit, after=after),
        )

    @group.command("cancel")
    def cancel(ctx: typer.Context, namespace_id: str, task_id: str) -> None:
        invoke(ctx, lambda api: api.cancel_tuning(namespace_id, task_id))

    @group.command("profile")
    def profile(ctx: typer.Context, namespace_id: str) -> None:
        invoke(ctx, lambda api: api.retrieval_profile(namespace_id))

    @group.command("apply")
    def apply(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, ApplyRetrievalProfile)
        invoke(ctx, lambda api: api.apply_retrieval_profile(namespace_id, command))

    @group.command("rollback")
    def rollback(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, RollbackRetrievalProfile)
        invoke(ctx, lambda api: api.rollback_retrieval_profile(namespace_id, command))
