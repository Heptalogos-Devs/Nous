"""Registered external documents use the same read and version operations as HTTP."""

from pathlib import Path
from typing import Literal

import typer
from nous_sdk.contracts.resources import ChangeResource, ReadResource, RegisterResource

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(
        help="Register external files and HTTP documents; inspect their saved versions."
    )
    cli.add_typer(group, name="resource")

    @group.command("register")
    def register_resource(ctx: typer.Context, namespace: str, subject: str, request: Path) -> None:
        command = read_command(request, RegisterResource)
        invoke(ctx, lambda api: api.register_resource(namespace, command))

    @group.command("list")
    def list_resources(
        ctx: typer.Context,
        namespace: str,
        subject: str,
        cursor: str | None = None,
        limit: int = 100,
    ) -> None:
        invoke(ctx, lambda api: api.resources(namespace, cursor=cursor, limit=limit))

    @group.command("get")
    def get_resource(
        ctx: typer.Context,
        namespace: str,
        subject: str,
        resource_id: str,
        revision: int | None = None,
    ) -> None:
        invoke(ctx, lambda api: api.resource(namespace, resource_id, revision=revision))

    @group.command("read")
    def read_resource(
        ctx: typer.Context,
        namespace: str,
        subject: str,
        resource_id: str,
        current: Literal["none", "prefer", "required"] = "none",
    ) -> None:
        invoke(
            ctx,
            lambda api: api.read_resource(
                namespace, ReadResource(resource_id=resource_id, current=current)
            ),
        )

    @group.command("change")
    def change_resource(ctx: typer.Context, namespace: str, subject: str, request: Path) -> None:
        command = read_command(request, ChangeResource)
        invoke(ctx, lambda api: api.change_resource(namespace, command))
