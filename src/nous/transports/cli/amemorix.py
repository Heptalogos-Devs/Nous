"""Run public A-Memorix operations through their SDK file and transport boundary."""

from pathlib import Path

import typer
from nous_sdk import NousClient
from nous_sdk.amemorix import AMemorixAdapter
from nous_sdk.amemorix.contracts import AMemorixOperation

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    @cli.command("amemorix")
    def execute(
        ctx: typer.Context,
        namespace_id: str,
        request: Path,
        transfer_root: Path | None = None,
        timeout_ms: int | None = None,
    ) -> None:
        """Read an AMemorixOperation JSON file containing tool and arguments."""
        command = read_command(request, AMemorixOperation)

        async def run(api: NousClient):
            adapter = AMemorixAdapter(api, namespace_id=namespace_id, transfer_root=transfer_root)
            return await adapter.invoke(command.tool, command.arguments, timeout_ms=timeout_ms)

        invoke(ctx, run)
