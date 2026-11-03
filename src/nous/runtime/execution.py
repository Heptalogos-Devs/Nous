"""Bounded asyncio operations and actual blocking-worker lifetime ownership."""

import asyncio
import contextvars
import functools
import logging
import time
from collections.abc import AsyncGenerator, Awaitable, Callable, Coroutine, Mapping
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager
from dataclasses import dataclass
from types import MappingProxyType
from typing import TypeVar, cast
from uuid import uuid4

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.configuration.definitions import ExecutionConfig
from nous.observability.instrumentation import get_meter, operation

T = TypeVar("T")


@dataclass(frozen=True)
class OperationContext:
    operation_id: str
    name: str
    settings: ConfigSnapshot
    deadline: float | None


_current: contextvars.ContextVar[Mapping[object, OperationContext]] = contextvars.ContextVar(
    "nous_operations", default=MappingProxyType({})
)


class ExecutionManager:
    """Own operation admission, inherited context and the workers needed to close safely."""

    def __init__(self, settings: ExecutionConfig, snapshot: Callable[[], ConfigSnapshot]) -> None:
        self._semaphore = asyncio.Semaphore(int(settings.max_in_flight))
        self._workers = ThreadPoolExecutor(
            max_workers=int(settings.max_blocking_workers), thread_name_prefix="nous-worker"
        )
        self._snapshot = snapshot
        self._owner = object()
        self._operations: dict[asyncio.Task[object], asyncio.Future[None]] = {}
        self._tasks: set[asyncio.Task[object]] = set()
        self._blocking: set[asyncio.Future[object]] = set()
        self._accepting = True

    @property
    def in_flight(self) -> int:
        return len(self._tasks | self._operations.keys())

    @property
    def snapshot(self) -> ConfigSnapshot:
        current = _current.get().get(self._owner)
        return current.settings if current is not None else self._snapshot()

    @asynccontextmanager
    async def operation(
        self, name: str, settings: ConfigSnapshot | None = None, *, deadline: float | None = None
    ) -> AsyncGenerator[OperationContext]:
        """Acquire once at the root; nested stages share its snapshot and cannot extend its deadline."""
        parent = _current.get().get(self._owner)
        task = asyncio.current_task()
        if not self._accepting and parent is None and task not in self._tasks:
            raise NousError("operation.unavailable", "The runtime is stopping.", operation=name)
        if parent is not None and settings is not None and settings is not parent.settings:
            raise NousError(
                "operation.snapshot_conflict",
                "A nested operation must use its parent's configuration snapshot.",
                operation=name,
            )
        snapshot = (
            parent.settings
            if parent is not None
            else (settings if settings is not None else self._snapshot())
        )
        if parent and parent.deadline is not None:
            deadline = min(deadline, parent.deadline) if deadline is not None else parent.deadline
        elif parent is None and deadline is None:
            timeout = snapshot.typed_values().execution.default_timeout_seconds
            if timeout is not None:
                deadline = asyncio.get_running_loop().time() + float(timeout)
        context = OperationContext(str(uuid4()), name, snapshot, deadline)
        root = parent is None
        completed = None
        if root and task is not None:
            completed = asyncio.get_running_loop().create_future()
            self._operations[task] = completed
        acquired = False
        token = None
        try:
            with operation(name, operation_id=context.operation_id):
                async with asyncio.timeout_at(deadline):
                    started = time.monotonic()
                    if root:
                        await self._semaphore.acquire()
                        acquired = True
                        get_meter("nous.execution").create_histogram(
                            "execution.queue_wait", unit="s"
                        ).record(time.monotonic() - started)
                    token = _current.set(MappingProxyType({**_current.get(), self._owner: context}))
                    yield context
        finally:
            if token is not None:
                _current.reset(token)
            if acquired:
                self._semaphore.release()
            if root and task is not None:
                self._operations.pop(task, None)
                if completed is not None and not completed.done():
                    completed.set_result(None)

    async def run(
        self, name: str, action: Callable[[], Awaitable[T]], *, deadline: float | None = None
    ) -> T:
        async with self.operation(name, deadline=deadline):
            return await action()

    def spawn(
        self, name: str, action: Callable[[], Coroutine[object, object, T]]
    ) -> asyncio.Task[T]:
        if not self._accepting:
            raise NousError("operation.unavailable", "The runtime is stopping.", operation=name)
        # A background operation owns a new independent request boundary while
        # retaining the OTel initiating context and other contextvars.
        context = contextvars.copy_context()
        context.run(_current.set, MappingProxyType({}))
        task = asyncio.create_task(self.run(name, action), name=name, context=context)
        self._tasks.add(task)

        def completed(done: asyncio.Task[T]) -> None:
            self._tasks.discard(done)
            if not done.cancelled() and (error := done.exception()) is not None:
                logging.getLogger(__name__).error(
                    "execution.background_failed", extra={"error_type": type(error).__name__}
                )

        task.add_done_callback(completed)
        return task

    async def blocking(
        self,
        action: Callable[..., T],
        *args: object,
        on_cancel: Callable[[], None] | None = None,
        **kwargs: object,
    ) -> T:
        """Keep a worker owned after caller cancellation until the actual work finishes."""
        if not self._accepting and self._owner not in _current.get():
            raise NousError("operation.unavailable", "The runtime is stopping.")
        context = contextvars.copy_context()
        future = asyncio.get_running_loop().run_in_executor(
            self._workers, context.run, functools.partial(action, *args, **kwargs)
        )
        self._blocking.add(cast(asyncio.Future[object], future))

        def completed(done: asyncio.Future[T]) -> None:
            self._blocking.discard(cast(asyncio.Future[object], done))
            if not done.cancelled():
                done.exception()  # Consume failures even after the waiter was cancelled.

        future.add_done_callback(completed)
        try:
            return await asyncio.shield(future)
        except asyncio.CancelledError as cancelled:
            if on_cancel is not None:
                try:
                    on_cancel()
                except Exception as error:
                    cancelled.add_note(f"Cancellation callback failed: {type(error).__name__}")
            # Resources held by the caller remain valid until the actual worker relinquishes them.
            while not future.done():
                try:
                    await asyncio.shield(future)
                except asyncio.CancelledError:
                    continue
                except Exception:
                    break
            if not future.cancelled() and (failure := future.exception()) is not None:
                code = failure.code if isinstance(failure, NousError) else type(failure).__name__
                cancelled.add_note(f"Blocking work failed during cancellation: {code}")
            raise cancelled

    async def aclose(self, grace_seconds: float = 30) -> None:
        """Stop admission, cancel overdue operations and await remaining blocking work."""
        self._accepting = False
        current = asyncio.current_task()
        tasks = self._tasks - {current}
        operations = {
            task: completed for task, completed in self._operations.items() if task is not current
        }
        waiters = [*tasks, *operations.values()]
        if waiters:
            await asyncio.wait(waiters, timeout=grace_seconds)
            for task, completed in operations.items():
                if not completed.done():
                    task.cancel()
            for task in tasks:
                if not task.done():
                    task.cancel()
            await asyncio.gather(
                *(asyncio.shield(cast(Awaitable[object], waiter)) for waiter in waiters),
                return_exceptions=True,
            )
        if self._blocking:
            logging.getLogger(__name__).info("execution.waiting_for_blocking_workers")
            await asyncio.gather(
                *(asyncio.shield(future) for future in tuple(self._blocking)),
                return_exceptions=True,
            )
        self._workers.shutdown(wait=True, cancel_futures=True)
