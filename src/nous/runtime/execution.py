"""Own this runtime's operations and the lifetime of its actual resources."""

import asyncio
import contextvars
import functools
from collections import OrderedDict
from collections.abc import AsyncGenerator, Awaitable, Callable, Coroutine, Mapping
from concurrent.futures import ThreadPoolExecutor
from contextlib import AbstractContextManager, asynccontextmanager, nullcontext
from dataclasses import dataclass
from types import MappingProxyType
from typing import Literal, TypeVar, cast
from uuid import uuid4

import structlog
from pydantic import JsonValue
from pydantic_core import to_jsonable_python

from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import OperationDiagnostics, OperationIssue, current_operation
from nous.configuration import ConfigSnapshot
from nous.configuration.definitions import ExecutionConfig
from nous.observability.instrumentation import get_logger, operation

T = TypeVar("T")


@dataclass(frozen=True)
class OperationContext:
    operation_id: str
    name: str
    settings: ConfigSnapshot
    deadline: float | None
    diagnostics: OperationDiagnostics
    stage: str


@dataclass(frozen=True)
class RunningOperationRef:
    """Identify an operation only for the lifetime of this runtime."""

    instance_id: str
    startup_id: str
    operation_id: str


@dataclass(frozen=True)
class RunningOperation:
    ref: RunningOperationRef
    name: str
    target_refs: tuple[object, ...]
    status: Literal["running", "succeeded", "failed", "cancelled"]
    completed_refs: tuple[dict[str, JsonValue], ...] = ()
    issue: OperationIssue | None = None
    outcome: JsonValue | None = None


@dataclass
class _ActiveOperation:
    view: RunningOperation
    task: asyncio.Task[object]
    diagnostics: OperationDiagnostics | None = None


_current: contextvars.ContextVar[Mapping[object, OperationContext]] = contextvars.ContextVar(
    "nous_operations", default=MappingProxyType({})
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


async def acquire_owned[T](work: Awaitable[T], release: Callable[[T], Awaitable[object]]) -> T:
    """Acquire a resource or release it if cancellation interrupts its transfer."""
    task = asyncio.ensure_future(work)
    try:
        return await asyncio.shield(task)
    except asyncio.CancelledError as cancelled:

        async def release_late() -> None:
            try:
                resource = await task
            except BaseException:
                return
            await release(resource)

        try:
            await complete_cleanup(release_late())
        except asyncio.CancelledError:
            pass
        except BaseException as error:
            cancelled.add_note(f"Resource cleanup failed: {type(error).__name__}: {error}")
        raise


async def stop_process(process: asyncio.subprocess.Process) -> None:
    """Terminate and reap an owned process, draining its pipes."""
    if process.returncode is None:
        try:
            process.kill()
        except ProcessLookupError:
            pass
    if process.stdin is not None:
        process.stdin.close()
    readers = [stream.read() for stream in (process.stdout, process.stderr) if stream is not None]
    await asyncio.gather(*readers, process.wait(), return_exceptions=True)


class ExecutionManager:
    """Own request contexts, background tasks and blocking-worker lifetimes.

    Nested stages share the root snapshot and deadline. Resource owners bound
    their own capacity; a waiting request holds no global business slot.
    """

    def __init__(
        self,
        settings: ExecutionConfig,
        snapshot: Callable[[], ConfigSnapshot],
        *,
        observability_scope: Callable[[], AbstractContextManager[None]] = nullcontext,
        allow_background: bool = False,
    ) -> None:
        self._background_limit = int(settings.max_background_tasks)
        self._allow_background = allow_background
        self._startup_id = str(uuid4())
        self._workers = ThreadPoolExecutor(
            max_workers=int(settings.max_blocking_workers), thread_name_prefix="nous-worker"
        )
        self._snapshot = snapshot
        self._observability_scope = observability_scope
        self._owner = object()
        self._operations: dict[asyncio.Task[object], asyncio.Future[None]] = {}
        self._tasks: set[asyncio.Task[object]] = set()
        self._active: dict[str, _ActiveOperation] = {}
        self._finished: OrderedDict[str, RunningOperation] = OrderedDict()
        self._controls: dict[asyncio.Task[object], tuple[int, asyncio.Future[None]]] = {}
        self._blocking: set[asyncio.Future[object]] = set()
        self._accepting = True
        self._closed = False

    @property
    def in_flight(self) -> int:
        return len(self._tasks | self._operations.keys() | self._controls.keys())

    @property
    def snapshot(self) -> ConfigSnapshot:
        current = _current.get().get(self._owner)
        return current.settings if current is not None else self._snapshot()

    def status(self) -> dict[str, object]:
        return {
            "root_operations": self.in_flight,
            "background_running": len(self._active),
            "blocking_workers": len(self._blocking),
            "accepting": self._accepting,
        }

    def _resolve(
        self, name: str, settings: ConfigSnapshot | None, deadline: float | None
    ) -> tuple[OperationContext | None, ConfigSnapshot, float | None]:
        parent = _current.get().get(self._owner)
        if parent is not None and settings is not None and settings is not parent.settings:
            raise NousError(
                "operation.snapshot_conflict",
                "A nested operation must use its parent's configuration snapshot.",
                operation=name,
            )
        snapshot = parent.settings if parent is not None else (settings or self._snapshot())
        if parent and parent.deadline is not None:
            deadline = min(deadline, parent.deadline) if deadline is not None else parent.deadline
        elif parent is None and deadline is None:
            timeout = snapshot.values.execution.default_timeout_seconds
            if timeout is not None:
                deadline = asyncio.get_running_loop().time() + float(timeout)
        return parent, snapshot, deadline

    @asynccontextmanager
    async def request(
        self,
        name: str,
        snapshot: ConfigSnapshot | None = None,
        timeout: float | None = None,  # noqa: ASYNC109
    ) -> AsyncGenerator[OperationContext]:
        """Capture one request context before input acquisition and business work."""
        if timeout is not None and timeout <= 0:
            raise NousError("input.invalid_timeout", "Timeout must be positive.")
        deadline = asyncio.get_running_loop().time() + timeout if timeout is not None else None
        async with self.operation(name, snapshot, deadline=deadline) as context:
            yield context

    @asynccontextmanager
    async def stage(self, context: OperationContext, name: str) -> AsyncGenerator[OperationContext]:
        """Enter a stage belonging to the currently active operation."""
        current = _current.get().get(self._owner)
        if current is None or current.operation_id != context.operation_id:
            raise NousError("operation.context_invalid", "Operation context is not active.")
        async with self.operation(name, context.settings, deadline=context.deadline) as stage:
            yield stage

    @asynccontextmanager
    async def operation(
        self,
        name: str,
        settings: ConfigSnapshot | None = None,
        *,
        deadline: float | None = None,
        _operation_id: str | None = None,
    ) -> AsyncGenerator[OperationContext]:
        """Enter an operation or nested stage using the inherited snapshot.

        Args:
            name: Name of this stage in operation diagnostics.
            settings: Root snapshot; nested stages must pass the identical object
                if supplied. None captures the current snapshot at the root.
            deadline: Absolute event-loop monotonic time in seconds. None uses the
                configured root timeout or inherits the parent's deadline. Nested
                stages can shorten, but cannot extend, the parent deadline.

        Raises:
            NousError: Admission has stopped or a nested snapshot conflicts.
            TimeoutError: Work exceeded the effective deadline.
        """
        parent = _current.get().get(self._owner)
        task = asyncio.current_task()
        if (
            not self._accepting
            and parent is None
            and task not in self._tasks
            and task not in self._controls
        ):
            raise NousError("operation.unavailable", "The runtime is stopping.", operation=name)
        parent, snapshot, deadline = self._resolve(name, settings, deadline)
        diagnostic = (
            parent.diagnostics
            if parent
            else OperationDiagnostics(_operation_id or str(uuid4()), name)
        )
        stage = f"{parent.stage}/{name}" if parent else name
        context = OperationContext(
            diagnostic.operation_id, name, snapshot, deadline, diagnostic, stage
        )
        root = parent is None
        completed = None
        if root and task is not None:
            completed = asyncio.get_running_loop().create_future()
            self._operations[task] = completed
        token = None
        try:
            with (
                self._observability_scope() if root else nullcontext(),
                operation(name, diagnostics=diagnostic, stage=stage, root_operation=root),
            ):
                async with asyncio.timeout_at(deadline):
                    token = _current.set(MappingProxyType({**_current.get(), self._owner: context}))
                    yield context
        finally:
            if token is not None:
                _current.reset(token)
            if root and task is not None:
                self._operations.pop(task, None)
                if completed is not None and not completed.done():
                    completed.set_result(None)

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
                and self._owner not in _current.get()
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

    def require_background(self) -> None:
        """Reject explicit background use before a write when no host owns its task."""
        if not self._allow_background:
            raise NousError(
                "execution.background_unavailable", "This runtime does not accept background work."
            )

    def spawn(
        self,
        name: str,
        action: Callable[[OperationContext], Coroutine[object, object, T]],
        *,
        target_refs: tuple[object, ...],
        snapshot: ConfigSnapshot | None = None,
    ) -> RunningOperationRef:
        """Start an independent, bounded in-process task and return its runtime reference."""
        self.require_background()
        if not self._accepting:
            raise NousError("operation.unavailable", "The runtime is stopping.", operation=name)
        if len(self._active) >= self._background_limit:
            raise NousError(
                "execution.background_capacity", "The runtime background task limit is reached."
            )
        settings = snapshot or self.snapshot
        initiating = _current.get().get(self._owner)
        ref = RunningOperationRef(settings.values.instance.id, self._startup_id, str(uuid4()))
        view = RunningOperation(ref, name, target_refs, "running")
        task_context = contextvars.copy_context()
        task_context.run(_current.set, MappingProxyType({}))
        task_context.run(current_operation.set, None)
        task_context.run(structlog.contextvars.unbind_contextvars, "initiating_operation")
        if initiating is not None:
            task_context.run(
                structlog.contextvars.bind_contextvars,
                initiating_operation=initiating.operation_id,
            )

        async def execute() -> T:
            async with self.operation(name, settings, _operation_id=ref.operation_id) as context:
                active.diagnostics = context.diagnostics
                return await action(context)

        task = cast(
            asyncio.Task[object], asyncio.create_task(execute(), name=name, context=task_context)
        )
        active = _ActiveOperation(view, task)
        self._active[ref.operation_id] = active
        self._tasks.add(task)
        task.add_done_callback(lambda completed: self._finish(active, completed))
        return ref

    def _finish(self, active: _ActiveOperation, task: asyncio.Task[object]) -> None:
        ref = active.view.ref
        if self._active.pop(ref.operation_id, None) is None:
            return
        self._tasks.discard(task)
        completed_refs = tuple(active.diagnostics.completed_refs) if active.diagnostics else ()
        issue = None
        outcome = None
        if task.cancelled():
            status = "cancelled"
        elif (error := task.exception()) is not None:
            status = "failed"
            issue = OperationIssue(
                stage=active.view.name,
                code=failure_code(error),
                message=failure_message(error),
                completed_refs=completed_refs,
            )
        else:
            status = "succeeded"
            outcome = to_jsonable_python(task.result())
        self._finished[ref.operation_id] = RunningOperation(
            ref, active.view.name, active.view.target_refs, status, completed_refs, issue, outcome
        )
        while len(self._finished) > 64:
            self._finished.popitem(last=False)

    def read_operation(self, ref: RunningOperationRef) -> RunningOperation:
        if (
            ref.instance_id != self._snapshot().values.instance.id
            or ref.startup_id != self._startup_id
        ):
            raise NousError("operation.expired", "This runtime operation is no longer available.")
        active = self._active.get(ref.operation_id)
        if active is not None:
            return active.view
        if (finished := self._finished.get(ref.operation_id)) is not None:
            return finished
        raise NousError("operation.expired", "This runtime operation is no longer available.")

    def list_operations(self) -> tuple[RunningOperation, ...]:
        return (*self._finished.values(), *(active.view for active in self._active.values()))

    async def wait(self, ref: RunningOperationRef) -> RunningOperation:
        self.read_operation(ref)
        if (active := self._active.get(ref.operation_id)) is not None:
            try:
                await asyncio.shield(active.task)
            except asyncio.CancelledError:
                if not active.task.cancelled():
                    raise
            except Exception:
                pass  # The terminal view carries the classified failure.
            self._finish(active, active.task)
        return self.read_operation(ref)

    async def cancel(self, ref: RunningOperationRef) -> RunningOperation:
        self.read_operation(ref)
        if (active := self._active.get(ref.operation_id)) is not None:
            active.task.cancel()
        return await self.wait(ref)

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
        tasks = self._tasks - {caller}
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
