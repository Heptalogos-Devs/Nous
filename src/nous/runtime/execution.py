"""Bounded asyncio operations and actual blocking-worker lifetime ownership."""

import asyncio
import contextvars
import functools
import time
from collections.abc import AsyncGenerator, Awaitable, Callable, Coroutine, Mapping
from concurrent.futures import ThreadPoolExecutor
from contextlib import AbstractContextManager, asynccontextmanager, nullcontext
from dataclasses import dataclass
from types import MappingProxyType
from typing import TypeVar, cast
from uuid import uuid4

from nous.common.errors import NousError
from nous.common.operations import OperationDiagnostics
from nous.configuration import ConfigSnapshot
from nous.configuration.definitions import ExecutionConfig
from nous.observability.instrumentation import get_logger, get_meter, operation

T = TypeVar("T")


@dataclass(frozen=True)
class OperationContext:
    operation_id: str
    name: str
    settings: ConfigSnapshot
    deadline: float | None
    diagnostics: OperationDiagnostics
    stage: str


_current: contextvars.ContextVar[Mapping[object, OperationContext]] = contextvars.ContextVar(
    "nous_operations", default=MappingProxyType({})
)
_admitted: contextvars.ContextVar[frozenset[object]] = contextvars.ContextVar(
    "nous_admitted_durable", default=frozenset()
)


async def complete_cleanup[T](work: Awaitable[T]) -> T:
    """Finish owned cleanup despite repeated cancellation, then propagate cancellation."""
    task = asyncio.ensure_future(work)
    cancelled: asyncio.CancelledError | None = None
    while not task.done():
        try:
            await asyncio.shield(task)
        except asyncio.CancelledError as error:
            if cancelled is None:
                cancelled = error
        except BaseException:
            break
    if cancelled is not None:
        if not task.cancelled() and (failure := task.exception()) is not None:
            cancelled.add_note(f"Cleanup failed: {type(failure).__name__}: {failure}")
        raise cancelled
    return task.result()


class ExecutionManager:
    """Own operation admission, inherited context and blocking-worker lifetimes.

    The runtime owns this manager and closes it after stopping admission. Root
    operations use a bounded slot; nested stages share the root configuration
    snapshot and deadline. Borrowers must finish using operation resources before
    the owning runtime closes.
    """

    def __init__(
        self,
        settings: ExecutionConfig,
        snapshot: Callable[[], ConfigSnapshot],
        *,
        observability_scope: Callable[[], AbstractContextManager[None]] = nullcontext,
    ) -> None:
        self._semaphore = asyncio.Semaphore(int(settings.max_in_flight))
        self._workers = ThreadPoolExecutor(
            max_workers=int(settings.max_blocking_workers), thread_name_prefix="nous-worker"
        )
        self._snapshot = snapshot
        self._observability_scope = observability_scope
        self._owner = object()
        self._operations: dict[asyncio.Task[object], asyncio.Future[None]] = {}
        self._tasks: set[asyncio.Task[object]] = set()
        self._durable: set[asyncio.Task[object]] = set()
        self._controls: dict[asyncio.Task[object], tuple[int, asyncio.Future[None]]] = {}
        self._blocking: set[asyncio.Future[object]] = set()
        self._accepting = True
        self._queued = 0
        self._running = 0
        self._closed = False

    @property
    def in_flight(self) -> int:
        return len(self._tasks | self._durable | self._operations.keys() | self._controls.keys())

    @property
    def snapshot(self) -> ConfigSnapshot:
        current = _current.get().get(self._owner)
        return current.settings if current is not None else self._snapshot()

    def status(self) -> dict[str, object]:
        return {
            "root_operations": self.in_flight,
            "queued": self._queued,
            "running": self._running,
            "blocking_workers": len(self._blocking),
            "durable_tasks": len(self._durable),
            "accepting": self._accepting,
        }

    @asynccontextmanager
    async def operation(
        self, name: str, settings: ConfigSnapshot | None = None, *, deadline: float | None = None
    ) -> AsyncGenerator[OperationContext]:
        """Enter an operation using one root slot and an inherited snapshot.

        Args:
            name: Name of this stage in operation diagnostics.
            settings: Root snapshot; nested stages must pass the identical object
                if supplied. None captures the current snapshot at the root.
            deadline: Absolute event-loop monotonic time in seconds. None uses the
                configured root timeout or inherits the parent's deadline. Nested
                stages can shorten, but cannot extend, the parent deadline.

        Raises:
            NousError: Admission has stopped or a nested snapshot conflicts.
            TimeoutError: Admission or work exceeded the effective deadline.
        """
        parent = _current.get().get(self._owner)
        task = asyncio.current_task()
        if (
            not self._accepting
            and parent is None
            and task not in self._tasks
            and task not in self._durable
            and task not in self._controls
            and self._owner not in _admitted.get()
        ):
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
            timeout = snapshot.values.execution.default_timeout_seconds
            if timeout is not None:
                deadline = asyncio.get_running_loop().time() + float(timeout)
        diagnostic = parent.diagnostics if parent else OperationDiagnostics(str(uuid4()), name)
        stage = f"{parent.stage}/{name}" if parent else name
        context = OperationContext(
            diagnostic.operation_id, name, snapshot, deadline, diagnostic, stage
        )
        root = parent is None
        completed = None
        if root and task is not None:
            completed = asyncio.get_running_loop().create_future()
            self._operations[task] = completed
        acquired = False
        token = None
        try:
            with (
                self._observability_scope() if root else nullcontext(),
                operation(name, diagnostics=diagnostic, stage=stage, root_operation=root),
            ):
                async with asyncio.timeout_at(deadline):
                    started = time.monotonic()
                    if root:
                        self._queued += 1
                        try:
                            await self._semaphore.acquire()
                        finally:
                            self._queued -= 1
                        acquired = True
                        self._running += 1
                        get_meter("nous.execution").create_histogram(
                            "execution.queue_wait", unit="s"
                        ).record(time.monotonic() - started)
                    token = _current.set(MappingProxyType({**_current.get(), self._owner: context}))
                    yield context
        finally:
            if token is not None:
                _current.reset(token)
            if acquired:
                self._running -= 1
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

    @asynccontextmanager
    async def control(self) -> AsyncGenerator[None]:
        """Track a slot-free control call until all its nested calls finish."""
        task = asyncio.current_task()
        if task is None:
            raise RuntimeError("Control requires an asyncio task")
        previous = self._controls.get(task)
        if previous is None:
            if (
                not self._accepting
                and task not in self._operations
                and task not in self._tasks
                and task not in self._durable
                and self._owner not in _admitted.get()
            ):
                raise NousError("operation.unavailable", "The runtime is stopping.")
            completed = asyncio.get_running_loop().create_future()
            self._controls[task] = (1, completed)
        else:
            completed = previous[1]
            self._controls[task] = (previous[0] + 1, completed)
        try:
            yield
        finally:
            count, _ = self._controls[task]
            if count == 1:
                del self._controls[task]
                completed.set_result(None)
            else:
                self._controls[task] = (count - 1, completed)

    @asynccontextmanager
    async def durable(self) -> AsyncGenerator[None]:
        """Track the framework's actual task through its final checkpoint."""
        task = asyncio.current_task()
        if task is None:
            raise RuntimeError("Durable execution requires an asyncio task")
        self._durable.add(task)
        task.add_done_callback(self._durable.discard)
        if not self._accepting:
            raise asyncio.CancelledError("Runtime is stopping")
        token = _current.set(MappingProxyType({}))
        admission = _admitted.set(_admitted.get() | {self._owner})
        try:
            yield
        finally:
            _admitted.reset(admission)
            _current.reset(token)

    def spawn(
        self, name: str, action: Callable[[], Coroutine[object, object, T]]
    ) -> asyncio.Task[T]:
        """Start an independently admitted root operation owned by this manager.

        The returned task can be awaited by the caller; shutdown also waits for
        it and cancels it after the grace period. Background failures are logged.
        """
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
                get_logger(__name__).error(
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
        """Run blocking work in the owned pool and wait for its actual completion.

        On cancellation, call ``on_cancel`` when provided, wait until the worker
        releases its resources, then propagate cancellation. The callback should
        request cooperative cancellation; it does not replace waiting for exit.
        """
        if not self._accepting and self._owner not in _current.get():
            raise NousError("operation.unavailable", "The runtime is stopping.")
        return await self.control_blocking(action, *args, on_cancel=on_cancel, **kwargs)

    async def control_blocking(
        self,
        action: Callable[..., T],
        *args: object,
        on_cancel: Callable[[], None] | None = None,
        **kwargs: object,
    ) -> T:
        """Run admitted control and shutdown work on the runtime-owned pool."""
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
        """Stop admission and close owned workers.

        Args:
            grace_seconds: Time in seconds to let admitted operations finish
                before cancelling them. Actual blocking workers are still awaited
                after cancellation, so shutdown may take longer than this grace.
        """
        await complete_cleanup(self._close(grace_seconds, asyncio.current_task()))

    def stop_admission(self) -> None:
        self._accepting = False

    async def drain(self, grace_seconds: float = 30) -> None:
        """Wait for accepted tasks and their actual cleanup while retaining the pool."""
        await complete_cleanup(self._drain(grace_seconds, asyncio.current_task()))

    async def _close(self, grace_seconds: float, caller: asyncio.Task[object] | None) -> None:
        if self._closed:
            return
        await self._drain(grace_seconds, caller)
        self._workers.shutdown(wait=True, cancel_futures=True)
        self._closed = True

    async def _drain(self, grace_seconds: float, caller: asyncio.Task[object] | None) -> None:
        self._accepting = False
        tasks = (self._tasks | self._durable) - {caller}
        operations = {
            task: completed for task, completed in self._operations.items() if task is not caller
        }
        controls = {
            task: completed for task, (_, completed) in self._controls.items() if task is not caller
        }
        waiters = [*tasks, *operations.values(), *controls.values()]
        if waiters:
            await asyncio.wait(waiters, timeout=grace_seconds)
            for task, completed in operations.items():
                if not completed.done():
                    task.cancel()
            for task, completed in controls.items():
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
            get_logger(__name__).info("execution.waiting_for_blocking_workers")
            await asyncio.gather(
                *(asyncio.shield(future) for future in tuple(self._blocking)),
                return_exceptions=True,
            )
