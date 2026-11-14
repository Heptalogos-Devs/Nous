"""Run a local host and its application on one owned asyncio loop."""

import asyncio
from collections.abc import AsyncGenerator, Awaitable, Callable, Mapping
from contextlib import AbstractAsyncContextManager, AsyncExitStack, asynccontextmanager
from pathlib import Path

from nous.credentials import Credentials
from nous.transports.shared import ServiceAddress

from .bootstrap import Runtime, open_instance
from .instance import InstanceLocator


@asynccontextmanager
async def serve_configured_transports(
    runtime: Runtime, locator: InstanceLocator
) -> AsyncGenerator[dict[str, ServiceAddress]]:
    """Start configured listeners within the host's runtime lifetime."""
    from nous.transports.grpc import serve_grpc
    from nous.transports.http import serve_http
    from nous.transports.mcp import serve_mcp

    config = runtime.configuration.snapshot.values
    credentials = Credentials(
        locator.directory, config.credentials, runtime.configuration.configuration.environment
    )

    def path(value: str | None) -> Path | None:
        if value is None:
            return None
        selected = Path(value).expanduser()
        return selected if selected.is_absolute() else locator.directory / selected

    addresses: dict[str, ServiceAddress] = {}
    async with AsyncExitStack() as stack:
        for name, serve in (("http", serve_http), ("mcp", serve_mcp), ("grpc", serve_grpc)):
            listener = getattr(config.transports, name)
            if not listener.enabled:
                continue
            assert listener.credential_ref is not None
            token = credentials.resolve(listener.credential_ref).get_secret_value()
            addresses[name] = await stack.enter_async_context(
                serve(
                    runtime.app,
                    runtime.artifacts,
                    token=token,
                    host=listener.host,
                    port=listener.port,
                    tls_certfile=path(listener.tls_certfile),
                    tls_keyfile=path(listener.tls_keyfile),
                )
            )
        yield addresses


def run_host[T](
    locator: InstanceLocator | Path | str | None,
    application: Callable[[Runtime], Awaitable[T]],
    *,
    environment: Mapping[str, str] | None = None,
    overrides: Mapping[str, object] | None = None,
    service: Callable[[Runtime], AbstractAsyncContextManager[object]] | None = None,
) -> T:
    """Own the application loop and its runtime-scoped background tasks."""

    async def run() -> T:
        async with open_instance(
            locator, environment=environment, overrides=overrides, allow_background=True
        ) as runtime:
            async with AsyncExitStack() as services:
                if service is not None:
                    await services.enter_async_context(service(runtime))
                return await application(runtime)

    with asyncio.Runner() as runner:
        return runner.run(run())
