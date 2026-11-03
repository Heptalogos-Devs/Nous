"""Export and install Nous Namespace bundles."""

from pathlib import Path

import anyio
import typer
from nous_sdk.contracts.management import BundleInstallation, NamespaceBundle
from nous_sdk.namespace import NamespaceClient

from .common import invoke_bound


def register(cli: typer.Typer) -> None:
    bundle = typer.Typer(help="Transfer Namespace knowledge and its sources.")
    cli.add_typer(bundle, name="bundle")

    @bundle.command("export")
    def export(ctx: typer.Context, destination: Path) -> None:
        """Export the current Namespace to a bundle file."""

        async def run(scope: NamespaceClient) -> dict[str, str | int]:
            value = await scope.bundles.export()
            _ = await anyio.Path(destination).write_text(value.model_dump_json(indent=2), encoding="utf-8")
            return {"path": str(destination), "id": value.id, "records": len(value.cognition)}

        invoke_bound(ctx, run)

    @bundle.command("install")
    def install(ctx: typer.Context, source: Path) -> None:
        """Install a bundle into the current Namespace."""

        async def run(scope: NamespaceClient) -> BundleInstallation:
            value = NamespaceBundle.model_validate_json(await anyio.Path(source).read_text(encoding="utf-8"))
            return await scope.bundles.install(value)

        invoke_bound(ctx, run)

    @bundle.command("list")
    def list_installations(ctx: typer.Context) -> None:
        """List bundles installed in the current Namespace."""
        invoke_bound(ctx, lambda scope: scope.bundles.installations())

    @bundle.command("uninstall")
    def uninstall(ctx: typer.Context, installation_id: str) -> None:
        """Remove the sources owned by an installed bundle."""
        invoke_bound(ctx, lambda scope: scope.bundles.uninstall(installation_id))
