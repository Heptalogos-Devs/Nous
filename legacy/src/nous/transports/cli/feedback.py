from pathlib import Path

import typer
from nous_sdk.contracts.feedback import (
    EnqueueFeedback,
    FeedbackQuery,
    RetryFeedback,
    RollbackFeedback,
)

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Schedule, inspect and roll back feedback corrections.")
    cli.add_typer(group, name="feedback")

    @group.command("retry")
    def retry(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, RetryFeedback)
        invoke(ctx, lambda api: api.retry_feedback(namespace_id, value))

    @group.command("enqueue")
    def enqueue(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, EnqueueFeedback)
        invoke(ctx, lambda api: api.enqueue_feedback(namespace_id, value))

    @group.command("get")
    def get(ctx: typer.Context, namespace_id: str, task_id: str) -> None:
        invoke(ctx, lambda api: api.feedback_task(namespace_id, task_id))

    @group.command("list")
    def list_tasks(ctx: typer.Context, namespace_id: str, command: Path | None = None) -> None:
        value = read_command(command, FeedbackQuery) if command else FeedbackQuery()
        invoke(ctx, lambda api: api.feedback_tasks(namespace_id, value))

    @group.command("rollback")
    def rollback(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, RollbackFeedback)
        invoke(ctx, lambda api: api.rollback_feedback(namespace_id, value))
