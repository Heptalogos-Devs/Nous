"""Entity identity changes share their source-backed typed domain operations."""

from pathlib import Path

import typer
from nous_sdk.contracts import MergeEntities, SplitEntity

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Merge and split Entity identities.", no_args_is_help=True)
    cli.add_typer(group, name="entities")

    @group.command("merge")
    def merge(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, MergeEntities)
        invoke(ctx, lambda api: api.merge_entities(namespace_id, value))

    @group.command("split")
    def split(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, SplitEntity)
        invoke(ctx, lambda api: api.split_entity(namespace_id, value))
