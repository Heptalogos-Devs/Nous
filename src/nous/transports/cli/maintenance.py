"""Reviewable growth and repeat policies through the public SDK."""

from pathlib import Path
from typing import Literal
from uuid import uuid4

import typer
from nous_wave.contracts.maintenance import (
    CreateMaintenanceSchedule,
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

    @group.command("start")
    def start(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        """Submit StartMaintenance JSON; generation invokes the configured provider."""
        value = read_command(command, StartMaintenance)
        invoke(ctx, lambda api: api.start_maintenance(namespace_id, subject_id, value))

    @group.command("list")
    def runs(ctx: typer.Context, namespace_id: str, subject_id: str, limit: int = 50) -> None:
        invoke(ctx, lambda api: api.maintenance_runs(namespace_id, subject_id, limit=limit))

    @group.command("read")
    def read(ctx: typer.Context, namespace_id: str, subject_id: str, run_id: str) -> None:
        invoke(ctx, lambda api: api.maintenance_run(namespace_id, subject_id, run_id))

    @group.command("review")
    def review(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        run_id: str,
        decision: Literal["adopt", "reject"],
        expected_revision: int,
        reason: str = "",
        operation_id: str | None = None,
    ) -> None:
        command = ReviewMaintenance(
            operation_id=operation_id or str(uuid4()),
            expected_revision=expected_revision,
            decision=decision,
            reason=reason,
        )
        invoke(ctx, lambda api: api.review_maintenance(namespace_id, subject_id, run_id, command))

    @group.command("schedule-start")
    def schedule(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        """Create a DBOS repeat policy from CreateMaintenanceSchedule JSON."""
        value = read_command(command, CreateMaintenanceSchedule)
        invoke(ctx, lambda api: api.create_maintenance_schedule(namespace_id, subject_id, value))

    @group.command("schedule-list")
    def schedules(ctx: typer.Context, namespace_id: str, subject_id: str) -> None:
        invoke(ctx, lambda api: api.maintenance_schedules(namespace_id, subject_id))

    @group.command("schedule-stop")
    def stop(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        schedule_id: str,
        operation_id: str | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.stop_maintenance_schedule(
                namespace_id, subject_id, schedule_id, operation_id=operation_id
            ),
        )
