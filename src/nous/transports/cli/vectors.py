from pathlib import Path

import typer
from nous_sdk.contracts.vector_spaces import (
    CheckVectorSpaces,
    DeleteVectorSpace,
    RebuildVectors,
    RecoverVectors,
    RetryVectorProjection,
)

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Inspect vector spaces and rebuild their derived projections.")
    cli.add_typer(group, name="vectors")

    @group.command("recover")
    def recover(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, RecoverVectors)
        invoke(ctx, lambda api: api.recover_vectors(namespace_id, value))

    @group.command("spaces")
    def spaces(
        ctx: typer.Context,
        namespace_id: str,
        include_media: bool = False,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.vector_spaces(namespace_id, include_media=include_media),
        )

    @group.command("delete-space")
    def delete_space(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, DeleteVectorSpace)
        invoke(ctx, lambda api: api.delete_vector_space(namespace_id, value))

    @group.command("rebuild")
    def rebuild(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, RebuildVectors)
        invoke(ctx, lambda api: api.rebuild_vectors(namespace_id, value))

    @group.command("check")
    def check(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, CheckVectorSpaces)
        invoke(ctx, lambda api: api.check_vector_spaces(namespace_id, value))

    @group.command("progress")
    def progress(ctx: typer.Context, namespace_id: str, request_id: str) -> None:
        invoke(ctx, lambda api: api.vector_projection_progress(namespace_id, request_id))

    @group.command("retry")
    def retry(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, RetryVectorProjection)
        invoke(ctx, lambda api: api.retry_vector_projection(namespace_id, value))

    @group.command("persistence")
    def persistence(ctx: typer.Context, namespace_id: str) -> None:
        invoke(ctx, lambda api: api.persistence_state(namespace_id))
