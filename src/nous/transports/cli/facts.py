"""Fact commands consume typed Native operations without a local claim store."""

from pathlib import Path

import typer
from nous_wave.contracts.facts import FactChange, FactQuery

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Source-backed claims, conflicts, validity and immutable transitions.")
    cli.add_typer(group, name="fact")

    @group.command("get")
    def get(ctx: typer.Context, namespace_id: str, subject_id: str, claim_id: str) -> None:
        invoke(ctx, lambda api: api.fact(namespace_id, subject_id, claim_id))

    @group.command("list")
    def query(ctx: typer.Context, namespace_id: str, subject_id: str, request: Path) -> None:
        command = read_command(request, FactQuery)
        invoke(ctx, lambda api: api.query_facts(namespace_id, subject_id, command))

    @group.command("change")
    def change(ctx: typer.Context, namespace_id: str, subject_id: str, request: Path) -> None:
        command = read_command(request, FactChange)
        invoke(ctx, lambda api: api.change_fact(namespace_id, subject_id, command))
