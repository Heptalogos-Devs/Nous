from pathlib import Path

import typer
from nous_sdk.contracts import CognitiveRef, RetentionChange, RetentionChanges
from nous_sdk.contracts.retention import DecayRetention

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Retain, protect, freeze and restore memories and associations.")
    cli.add_typer(group, name="retention")

    @group.command("decay")
    def decay(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, DecayRetention)
        invoke(ctx, lambda api: api.decay_retention(namespace_id, value))

    @group.command("change")
    def change(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, RetentionChange)
        invoke(ctx, lambda api: api.change_retention(namespace_id, value))

    @group.command("change-many")
    def change_many(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, RetentionChanges)
        invoke(ctx, lambda api: api.change_retentions(namespace_id, value))

    @group.command("snapshot")
    def snapshot(ctx: typer.Context, namespace_id: str, reference: Path) -> None:
        value = read_command(reference, CognitiveRef)
        invoke(ctx, lambda api: api.retention_snapshot(namespace_id, value))
