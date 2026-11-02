"""Inspect and edit the active TOML through the settings resource."""

from pathlib import Path
from typing import Annotated

import anyio
import click
import typer
from nous_sdk.settings import SettingsResource

from .common import invoke, options


def register(cli: typer.Typer) -> None:
    group = typer.Typer(
        help="Read, edit and reload Namespace or instance configuration.", no_args_is_help=True
    )
    cli.add_typer(group, name="config")

    async def resource(ctx, root, instance):
        if instance:
            return SettingsResource(root)
        namespace = options(ctx).namespace
        if namespace is None:
            raise ValueError("Choose a Namespace or use --instance-scope")
        return (await root.namespace(namespace)).settings

    @group.command("get")
    def get(
        ctx: typer.Context,
        key: Annotated[str | None, typer.Argument()] = None,
        instance_scope: bool = False,
    ):
        async def run(root):
            settings = await resource(ctx, root, instance_scope)
            return await settings.get(key) if key else {"text": await settings.export()}

        invoke(ctx, run)

    @group.command("export")
    def export(ctx: typer.Context, output: Path | None = None, instance_scope: bool = False):
        async def run(root):
            settings = await resource(ctx, root, instance_scope)
            text = await settings.export()
            if output is None:
                return {"text": text}
            async with await anyio.open_file(output, "x", encoding="utf-8") as stream:
                await stream.write(text)
            return {"file": str(output)}

        invoke(ctx, run)

    @group.command("edit")
    def edit(ctx: typer.Context, file: Path | None = None, instance_scope: bool = False):
        async def run(root):
            settings = await resource(ctx, root, instance_scope)
            original = await settings.export()
            updated = (
                await anyio.Path(file).read_text(encoding="utf-8")
                if file
                else await anyio.to_thread.run_sync(lambda: click.edit(original, extension=".toml"))
            )
            if updated is None or updated == original:
                return {"status": "unchanged"}
            return await settings.replace(updated)

        invoke(ctx, run)

    @group.command("reload")
    def reload(ctx: typer.Context, instance_scope: bool = False):
        async def run(root):
            return await (await resource(ctx, root, instance_scope)).reload()

        invoke(ctx, run)
