"""CLI composition over the SDK and instance lifecycle owners."""

import asyncio
import json
import re
import sys
from contextvars import ContextVar
from io import TextIOWrapper
from pathlib import Path
from typing import Annotated

import httpx
import typer
import uvicorn
from nous_wave import NousError as ClientError
from nous_wave.contracts import DomainError
from rich.console import Console

from nous.platform.configuration import Settings
from nous.platform.errors import NousError
from nous.platform.instance import initialize
from nous.platform.localization import Localizer

from .common import Options, emit, options
from .contexts import register as register_contexts
from .data import register as register_data
from .management import register as register_management
from .service import register as register_service

cli = typer.Typer(
    help="Nous Wave cognitive service", no_args_is_help=False, invoke_without_command=True
)
_current: ContextVar[Options | None] = ContextVar("cli_options", default=None)


@cli.callback()
def configure(
    ctx: typer.Context,
    instance: Annotated[Path, typer.Option(envvar="NOUS_INSTANCE")] = Path(
        "temp/instances/default"
    ),
    url: Annotated[str, typer.Option(envvar="NOUS_URL")] = "http://127.0.0.1:7441",
    credential_file: Annotated[Path | None, typer.Option(envvar="NOUS_CREDENTIAL_FILE")] = None,
    locale: Annotated[str | None, typer.Option(envvar="NOUS_LOCALE")] = None,
    json_mode: Annotated[
        bool, typer.Option("--json", help="Write exactly one JSON object.")
    ] = False,
    source_scope: Annotated[
        Path | None, typer.Option(help="JSON array of SDK SourceReadScope selectors.")
    ] = None,
) -> None:
    selected = Options(
        instance, url, credential_file, Localizer(locale).locale, json_mode, source_scope
    )
    ctx.obj = selected
    _current.set(selected)
    if ctx.invoked_subcommand is None:
        if json_mode:
            raise ValueError("A command is required; use --help to see available commands")
        typer.echo(ctx.get_help())
        raise typer.Exit()


@cli.command()
def init(ctx: typer.Context, instance: Path | None = None) -> None:
    """Initialize directories and the protected instance credential."""
    selected = instance or options(ctx).instance
    path = initialize(selected)
    emit(ctx, {"instance": str(selected.resolve()), "credential_file": str(path.resolve())})


@cli.command()
def serve(
    ctx: typer.Context, instance: Path | None = None, host: str = "127.0.0.1", port: int = 7441
) -> None:
    """Run the foreground HTTP host and its durable workers."""
    from nous.platform.observability import configure_logging
    from nous.transports.http import create_app

    if options(ctx).json_mode:
        raise ValueError("Foreground service logs are a stream; omit --json when serving")
    settings = Settings(instance=instance or options(ctx).instance, host=host, port=port)
    configure_logging(settings.log_level)
    uvicorn.run(create_app(settings), host=host, port=port, log_config=None)


@cli.command()
def mcp(
    ctx: typer.Context,
    namespace_id: Annotated[str, typer.Option(envvar="NOUS_NAMESPACE_ID")],
    subject_id: Annotated[str, typer.Option(envvar="NOUS_SUBJECT_ID")],
    instance: Path | None = None,
    url: str | None = None,
    credential_file: Path | None = None,
) -> None:
    """Bind MCP stdio to an authorized namespace and subject."""
    from nous.transports.mcp import run

    selected = options(ctx)
    run(
        instance or selected.instance,
        url or selected.url,
        namespace_id,
        subject_id,
        credential_file or selected.credential_file,
    )


@cli.command()
def restore(ctx: typer.Context, archive: Path, instance: Path) -> None:
    """Restore an archive into a new instance directory."""
    from nous.platform.persistence import BackupService

    emit(ctx, asyncio.run(BackupService.restore(archive, instance)))


register_data(cli)
register_management(cli)
register_contexts(cli)
register_service(cli)


def main() -> None:
    for stream in (sys.stdout, sys.stderr):
        if isinstance(stream, TextIOWrapper):
            stream.reconfigure(encoding="utf-8")
    try:
        cli(standalone_mode=False)
    except (
        ClientError,
        DomainError,
        NousError,
        ValueError,
        OSError,
        TimeoutError,
        typer.TyperException,
        httpx.HTTPError,
        typer.Abort,
        KeyboardInterrupt,
    ) as error:
        selected = _current.get()
        locale = Localizer(selected.locale if selected else None)
        code = (
            "Cancelled"
            if isinstance(error, (typer.Abort, KeyboardInterrupt))
            else (
                error.code
                if isinstance(error, (ClientError, DomainError, NousError))
                else "InvalidArgument"
                if isinstance(error, (ValueError, typer.TyperException))
                else "Unavailable"
            )
        )
        if (selected and selected.json_mode) or (selected is None and "--json" in sys.argv):
            sys.stdout.write(
                json.dumps({"error": {"code": code, "message": str(error)}}, ensure_ascii=False)
                + "\n"
            )
        else:
            key = "error-" + re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "-", code.replace("_", "-")).lower()
            category = (
                "error-forbidden"
                if "forbidden" in code.lower()
                else "error-unauthorized"
                if "credential" in code.lower()
                or (isinstance(error, ClientError) and error.status_code == 401)
                else "error-dependency-unavailable"
                if isinstance(error, (NousError, OSError, TimeoutError, httpx.HTTPError))
                else "error-invalid-argument"
            )
            Console(stderr=True).print(
                locale.text(key if locale.has(key) else category), style="red", markup=False
            )
            Console(stderr=True).print(str(error), markup=False)
        raise SystemExit(130 if code == "Cancelled" else 1) from None
