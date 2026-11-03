"""Saved correction plans can be adopted or rolled back from another client."""

from pathlib import Path

import typer
from nous_sdk.contracts.corrections import ApplyCorrection, PlanCorrection, RollbackCorrection

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Preview, apply and roll back cognitive corrections.")
    cli.add_typer(group, name="correction")

    @group.command("plan")
    def plan(ctx: typer.Context, namespace: str, subject: str, request: Path) -> None:
        """Prepare edits from a PlanCorrection JSON file, including natural language intent."""
        command = read_command(request, PlanCorrection)
        invoke(ctx, lambda api: api.plan_correction(namespace, command))

    @group.command("get")
    def get(ctx: typer.Context, namespace: str, subject: str, plan_id: str) -> None:
        invoke(ctx, lambda api: api.correction_plan(namespace, plan_id))

    @group.command("list")
    def list_plans(ctx: typer.Context, namespace: str, subject: str) -> None:
        invoke(ctx, lambda api: api.correction_plans(namespace))

    @group.command("apply")
    def apply(ctx: typer.Context, namespace: str, subject: str, plan_id: str) -> None:
        invoke(
            ctx,
            lambda api: api.apply_correction(namespace, ApplyCorrection(plan_id=plan_id)),
        )

    @group.command("rollback")
    def rollback(
        ctx: typer.Context,
        namespace: str,
        subject: str,
        plan_id: str,
        reason: str = "Rollback correction",
    ) -> None:
        invoke(
            ctx,
            lambda api: api.rollback_correction(
                namespace, RollbackCorrection(plan_id=plan_id, reason=reason)
            ),
        )
