"""Reviewable growth and repeat policies through the public SDK."""

from pathlib import Path
from typing import Annotated, Literal
from uuid import uuid4

import typer
from nous_sdk.contracts.maintenance import (
    CreateMaintenanceSchedule,
    EditMaintenanceProposal,
    ReviewMaintenance,
    StartMaintenance,
)

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(
        help="Generate, review and schedule evidence-backed cognitive growth.",
        no_args_is_help=False,
    )
    cli.add_typer(group, name="maintenance")

    @group.command("edit")
    def edit(ctx: typer.Context, namespace_id: str, run_id: str, command: Path) -> None:
        """Compile an explicitly edited proposal while retaining the original model call."""
        value = read_command(command, EditMaintenanceProposal)
        invoke(ctx, lambda api: api.edit_maintenance(namespace_id, run_id, value))

    @group.command("retry")
    def retry(
        ctx: typer.Context,
        namespace_id: str,
        run_id: str,
        operation_id: str | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.retry_maintenance(namespace_id, run_id, operation_id=operation_id),
        )

    @group.command("start")
    def start(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        """Submit StartMaintenance JSON; generation invokes the configured provider."""
        value = read_command(command, StartMaintenance)
        invoke(ctx, lambda api: api.start_maintenance(namespace_id, value))

    @group.command("list")
    def runs(ctx: typer.Context, namespace_id: str, limit: int = 50) -> None:
        invoke(ctx, lambda api: api.maintenance_runs(namespace_id, limit=limit))

    @group.command("read")
    def read(ctx: typer.Context, namespace_id: str, run_id: str) -> None:
        invoke(ctx, lambda api: api.maintenance_run(namespace_id, run_id))

    @group.command("review")
    def review(
        ctx: typer.Context,
        namespace_id: str,
        run_id: str,
        decision: Literal["adopt", "reject"],
        expected_revision: int,
        reason: str = "",
        operation_id: str | None = None,
        group_key: Annotated[list[str] | None, typer.Option("--group")] = None,
    ) -> None:
        command = ReviewMaintenance(
            operation_id=operation_id or str(uuid4()),
            expected_revision=expected_revision,
            decision=decision,
            reason=reason,
            group_keys=tuple(group_key) if group_key is not None else None,
        )
        invoke(ctx, lambda api: api.review_maintenance(namespace_id, run_id, command))

    @group.command("schedule-start")
    def schedule(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        """Create a DBOS repeat policy from CreateMaintenanceSchedule JSON."""
        value = read_command(command, CreateMaintenanceSchedule)
        invoke(ctx, lambda api: api.create_maintenance_schedule(namespace_id, value))

    @group.command("schedule-list")
    def schedules(ctx: typer.Context, namespace_id: str) -> None:
        invoke(ctx, lambda api: api.maintenance_schedules(namespace_id))

    @group.command("schedule-stop")
    def stop(
        ctx: typer.Context,
        namespace_id: str,
        schedule_id: str,
        operation_id: str | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.stop_maintenance_schedule(
                namespace_id, schedule_id, operation_id=operation_id
            ),
        )
