"""Social interpretations, explicit account corrections and bounded motivation reads."""

from pathlib import Path

import typer
from nous_sdk.contracts.motivation import AssessMotivation, MotivationAssessmentsQuery
from nous_sdk.contracts.social import (
    IdentityHypothesis,
    SymbolSelection,
    UnlinkIdentity,
)

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Read scoped social meanings and manage account correspondence.")
    cli.add_typer(group, name="social")

    @cli.command("motivation-history")
    def motivation_history(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        """Page saved assessments; request may filter an exact Desire/Concern revision."""
        command = read_command(request, MotivationAssessmentsQuery)
        invoke(ctx, lambda api: api.motivation_assessments(namespace_id, command))

    @cli.command("motivation-assessment")
    def motivation_assessment(ctx: typer.Context, namespace_id: str, assessment_id: str) -> None:
        invoke(ctx, lambda api: api.motivation_assessment(namespace_id, assessment_id))

    @group.command("hypothesize-identity")
    def hypothesis(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, IdentityHypothesis)
        invoke(ctx, lambda api: api.propose_identity(namespace_id, command))

    @group.command("unlink-identity")
    def unlink(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, UnlinkIdentity)
        invoke(ctx, lambda api: api.unlink_identity(namespace_id, command))

    @group.command("understand")
    def understand(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, SymbolSelection)
        invoke(ctx, lambda api: api.understand_symbol(namespace_id, command))

    @group.command("expressions")
    def expressions(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, SymbolSelection)
        invoke(ctx, lambda api: api.expression_candidates(namespace_id, command))

    @cli.command("assess-motivation")
    def motivation(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, AssessMotivation)
        invoke(ctx, lambda api: api.assess_motivation(namespace_id, command))
