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
from nous_sdk import NousError as ClientError
from nous_sdk.contracts import DomainError
from rich.console import Console

from nous.platform.configuration import Settings
from nous.platform.errors import NousError
from nous.platform.instance import initialize
from nous.platform.localization import Localizer
from nous.platform.paths import default_instance_path

from .client_state import ClientState, load, save
from .bundles import register as register_bundles
from .commands import register as register_commands
from .common import Options, emit, options
from .configuration import register as register_configuration
from .ego import register as register_ego
from .entities import register as register_entities
from .feedback import register as register_feedback
from .maintenance import register as register_maintenance
from .management import register as register_management
from .retention import register as register_retention
from .service import register as register_service
from .social import register as register_social
from .tuning import register as register_tuning
from .vectors import register as register_vectors

cli = typer.Typer(help="Nous cognitive service", no_args_is_help=False, invoke_without_command=True)
_current: ContextVar[Options | None] = ContextVar("cli_options", default=None)


@cli.callback()
def configure(
    ctx: typer.Context,
    instance: Annotated[Path | None, typer.Option(envvar="NOUS_INSTANCE")] = None,
    url: Annotated[str | None, typer.Option(envvar="NOUS_URL")] = None,
    credential_file: Annotated[Path | None, typer.Option(envvar="NOUS_CREDENTIAL_FILE")] = None,
    namespace: Annotated[str | None, typer.Option(envvar="NOUS_NAMESPACE")] = None,
    locale: Annotated[str | None, typer.Option(envvar="NOUS_LOCALE")] = None,
    json_mode: Annotated[
        bool, typer.Option("--json", help="Write the compact result as JSON.")
    ] = False,
    raw: Annotated[bool, typer.Option(help="Include the complete SDK result.")] = False,
    fields: Annotated[str | None, typer.Option(help="Comma separated result fields.")] = None,
) -> None:
    state, path = load()
    selected = Options(
        instance=instance or state.instance or default_instance_path(),
        url=url or state.url,
        credential_file=credential_file or state.credential_file,
        namespace=namespace or state.namespace,
        locale=Localizer(locale).locale,
        json_mode=json_mode,
        raw=raw,
        fields=tuple(field.strip() for field in (fields or "").split(",") if field.strip()),
        client_file=path,
        client_state=state,
    )
    ctx.obj = selected
    _current.set(selected)
    if ctx.invoked_subcommand is None:
        typer.echo(ctx.get_help())
        raise typer.Exit()


@cli.command()
def init(ctx: typer.Context, instance: Path | None = None, name: str = "default") -> None:
    """Create a local instance, credential and initial Namespace."""
    from uuid import uuid4

    from nous_sdk.contracts import CallContext, DomainError

    from nous.application import Application

    selected = options(ctx)
    location = instance or selected.instance
    credential = initialize(location)

    async def create():
        app = Application(Settings(instance=location))
        await app.authority.initialize()
        try:
            try:
                domain = await app.identity.resolve(name)
                if domain.slug != name:
                    domain = None
            except DomainError as error:
                if error.code != "namespace_not_found":
                    raise
                domain = None
            if domain is None:
                namespace_id = str(uuid4())
                context = CallContext(
                    caller_id="nous.init", namespace_id=namespace_id, credential_source_ids=None
                )
                domain = await app.identity.create_namespace(
                    context, name, "init:" + name, namespace_id
                )
            state = selected.client_state or ClientState()
            assert selected.client_file is not None
            save(
                selected.client_file,
                state.model_copy(
                    update={
                        "url": selected.url,
                        "instance": location.resolve(),
                        "credential_file": credential.resolve(),
                        "namespace": domain.id,
                    }
                ),
            )
            return {
                "instance": str(location.resolve()),
                "credential_file": str(credential.resolve()),
                "namespace": domain.slug,
                "next": "nous serve",
            }
        finally:
            await app.close()

    emit(ctx, asyncio.run(create()))


@cli.command()
def serve(
    ctx: typer.Context,
    instance: Path | None = None,
    host: str = "127.0.0.1",
    port: int = 7441,
) -> None:
    """Run the foreground HTTP host and its durable workers."""
    from nous.platform.observability import configure_logging
    from nous.transports.http import create_app

    if options(ctx).json_mode:
        raise ValueError("Foreground service logs are a stream; omit --json when serving")
    settings = Settings(instance=instance or options(ctx).instance, host=host, port=port)
    configure_logging(settings.log_level)
    app = create_app(settings)
    server = uvicorn.Server(
        uvicorn.Config(app, host=host, port=port, log_config=None, reset_contextvars=True)
    )

    def request_shutdown() -> None:
        server.should_exit = True

    app.state.request_shutdown = request_shutdown
    server.run()


@cli.command()
def mcp(ctx: typer.Context, read_only: bool = False) -> None:
    """Expose the selected Namespace as ordinary MCP tools over stdio."""
    from nous.transports.mcp import run

    selected = options(ctx)
    if selected.namespace is None:
        raise ValueError("Choose a Namespace with 'nous use NAME' or --namespace NAME")
    run(
        selected.instance,
        selected.url,
        selected.namespace,
        selected.credential_file,
        read_only=read_only,
    )


@cli.command()
def restore(ctx: typer.Context, archive: Path, instance: Path) -> None:
    """Restore an archive into a new instance directory."""
    from nous.platform.persistence import BackupService

    emit(ctx, asyncio.run(BackupService.restore(archive, instance)))


register_commands(cli)
register_bundles(cli)
register_configuration(cli)
admin = typer.Typer(help="Configure resources and manage cognitive data.", no_args_is_help=True)
cli.add_typer(admin, name="admin")
register_management(admin)
register_maintenance(admin)
register_ego(admin)
register_social(admin)
register_entities(admin)
register_feedback(admin)
register_vectors(admin)
register_retention(admin)
register_service(admin)
register_tuning(admin)


def main() -> None:
    from nous.platform.observability import configure_logging

    configure_logging("WARNING")
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
        details = (
            error.context
            if isinstance(error, (ClientError, NousError))
            else error.details
            if isinstance(error, DomainError)
            else {}
        )
        if (selected and selected.json_mode) or (selected is None and "--json" in sys.argv):
            sys.stdout.write(
                json.dumps(
                    {
                        "error": {
                            "code": code,
                            "message": str(error),
                            "context": details,
                            "trace_id": error.trace_id if isinstance(error, ClientError) else None,
                        }
                    },
                    ensure_ascii=False,
                )
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
            if details:
                Console(stderr=True).print(json.dumps(details, ensure_ascii=False), markup=False)
        status = (
            130
            if code == "Cancelled"
            else 3
            if any(term in code.lower() for term in ("forbidden", "unauthorized", "credential"))
            else 4
            if "not_found" in code.lower() or "notfound" in code.lower()
            else 6
            if "conflict" in code.lower()
            else 2
            if isinstance(error, (ValueError, typer.TyperException))
            else 5
        )
        raise SystemExit(status) from None
