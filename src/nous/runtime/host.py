"""Run a local host and its application on one owned asyncio loop."""

import asyncio
from collections.abc import Awaitable, Callable, Mapping
from contextlib import AbstractAsyncContextManager, AsyncExitStack
from pathlib import Path

from .bootstrap import Runtime, open_instance
from .instance import InstanceLocator


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
