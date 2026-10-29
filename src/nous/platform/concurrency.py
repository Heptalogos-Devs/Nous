"""Run blocking work without abandoning its resources when the caller cancels."""

import asyncio
from collections.abc import Callable


async def owned_thread[Result](
    call: Callable[[], Result], *, on_cancel: Callable[[], None] | None = None
) -> Result:
    work = asyncio.create_task(asyncio.to_thread(call))
    try:
        return await asyncio.shield(work)
    except asyncio.CancelledError as cancelled:
        try:
            if on_cancel is not None:
                on_cancel()
        finally:
            while not work.done():
                try:
                    await asyncio.shield(work)
                except asyncio.CancelledError:
                    continue
                except Exception:
                    break
            if not work.cancelled():
                work.exception()
        raise cancelled
