"""Run the local host and inspect its current background operations."""

import asyncio
from collections.abc import AsyncGenerator
from contextlib import AsyncExitStack, asynccontextmanager
from dataclasses import asdict

import typer

from nous.runtime.bootstrap import Runtime
from nous.runtime.host import run_host, serve_configured_transports
from nous.runtime.operations import (
    CANCEL_OPERATION,
    LIST_OPERATIONS,
    READ_OPERATION,
    WAIT_OPERATION,
    ListOperations,
    SelectOperation,
)

from .context import application_call, emit, options
from .groups import runtime_cli

operations_cli = typer.Typer(
    help="运行中的后台操作 / Current background operations", no_args_is_help=True
)
runtime_cli.add_typer(operations_cli, name="operations")


@runtime_cli.command("run")
def run(ctx: typer.Context) -> None:
    """Run the local instance host until interrupted."""
    from .local_control import serve_cli

    async def application(runtime: Runtime) -> None:
        await asyncio.Event().wait()

    locator = options(ctx).locator

    @asynccontextmanager
    async def services(runtime: Runtime) -> AsyncGenerator[None]:
        async with AsyncExitStack() as stack:
            await stack.enter_async_context(serve_cli(runtime, locator))
            addresses = await stack.enter_async_context(
                serve_configured_transports(runtime, locator)
            )
            for name, address in addresses.items():
                typer.echo(f"{name}: {address.host}:{address.port}")
            yield

    run_host(locator, application, service=services)


@operations_cli.command("list")
def list_operations(ctx: typer.Context) -> None:
    values = application_call(ctx, LIST_OPERATIONS, ListOperations())
    emit(ctx, {"operations": [asdict(value) for value in values]})


@operations_cli.command("read")
def read_operation(ctx: typer.Context, operation_id: str) -> None:
    emit(ctx, asdict(application_call(ctx, READ_OPERATION, SelectOperation(ref=operation_id))))


@operations_cli.command("wait")
def wait_operation(ctx: typer.Context, operation_id: str, timeout: float | None = None) -> None:
    emit(
        ctx,
        asdict(
            application_call(
                ctx, WAIT_OPERATION, SelectOperation(ref=operation_id), timeout=timeout
            )
        ),
    )


@operations_cli.command("cancel")
def cancel_operation(ctx: typer.Context, operation_id: str) -> None:
    emit(ctx, asdict(application_call(ctx, CANCEL_OPERATION, SelectOperation(ref=operation_id))))
