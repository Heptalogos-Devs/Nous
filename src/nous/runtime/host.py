"""Run an explicitly owned DBOS host and application on one asyncio loop."""

import asyncio
from collections.abc import Awaitable, Callable, Mapping
from pathlib import Path

from .bootstrap import Runtime, open_instance
from .execution import complete_cleanup
from .instance import InstanceLocator


def run_host[T](
    locator: InstanceLocator | Path | str | None,
    application: Callable[[Runtime], Awaitable[T]],
    *,
    environment: Mapping[str, str] | None = None,
    overrides: Mapping[str, object] | None = None,
) -> T:
    """Own the application loop, durable scheduler and their shutdown order."""

    async def run() -> T:
        async with open_instance(locator, environment=environment, overrides=overrides) as runtime:
            await runtime.job_runtime.start(runtime.maintenance)
            try:
                return await application(runtime)
            finally:

                async def stop_host() -> None:
                    runtime.execution.stop_admission()
                    await runtime.execution.drain(
                        runtime.configuration.snapshot.values.runtime.shutdown_grace_seconds
                    )
                    await runtime.job_runtime.close()
                    # Wrappers dispatched during shutdown cannot enter business work,
                    # but their actual tasks must still finish before resources close.
                    await runtime.execution.drain(0)

                await complete_cleanup(stop_host())

    with asyncio.Runner() as runner:
        return runner.run(run())
