"""Dispatch durable steps onto the runtime loop and await their actual exit."""

from __future__ import annotations

import asyncio
import contextvars
import threading
from collections.abc import Awaitable, Callable, Mapping
from concurrent.futures import Future
from dataclasses import dataclass, field

import structlog
from opentelemetry import context as otel_context
from opentelemetry.propagate import extract
from pydantic import JsonValue

from nous.runtime.execution import ExecutionManager

JsonObject = dict[str, JsonValue]
Stage = Callable[[JsonObject], Awaitable[JsonObject]]


@dataclass(eq=False)
class _Flight:
    done: Future[tuple[JsonObject | None, BaseException | None]] = field(default_factory=Future)
    cancelled: bool = False
    task: asyncio.Task[JsonObject] | None = None


class RuntimeBridge:
    """Own cross-loop dispatches without transferring a live runtime context."""

    def __init__(self, execution: ExecutionManager) -> None:
        self._loop = asyncio.get_running_loop()
        self._execution = execution
        self._lock = threading.Lock()
        self._accepting = True
        self._flights: set[_Flight] = set()

    @property
    def in_flight(self) -> int:
        with self._lock:
            return len(self._flights)

    async def call(
        self,
        operation: Stage,
        payload: JsonObject,
        *,
        association: Mapping[str, str],
        deadline: float | None = None,
    ) -> JsonObject:
        flight = _Flight()
        with self._lock:
            if not self._accepting:
                raise asyncio.CancelledError("Runtime is stopping")
            self._flights.add(flight)

        async def perform() -> JsonObject:
            carrier = {
                key: value
                for key, value in association.items()
                if key in {"traceparent", "tracestate"}
            }
            token = otel_context.attach(extract(carrier))
            try:
                with structlog.contextvars.bound_contextvars(
                    job_id=association.get("job_id"), job_step=association.get("step")
                ):
                    async with self._execution.operation(
                        f"jobs.{association.get('step', 'maintenance')}", deadline=deadline
                    ):
                        return await operation(payload)
            finally:
                otel_context.detach(token)

        def finish(task: asyncio.Task[JsonObject]) -> None:
            try:
                value = task.result()
                result = (value, None)
            except BaseException as error:
                result = (None, error)
            with self._lock:
                self._flights.discard(flight)
            flight.done.set_result(result)

        def start() -> None:
            with self._lock:
                if flight.cancelled or not self._accepting:
                    self._flights.discard(flight)
                    flight.done.set_result((None, asyncio.CancelledError("Runtime is stopping")))
                    return
                flight.task = self._loop.create_task(perform(), context=contextvars.Context())
                flight.task.add_done_callback(finish)

        self._loop.call_soon_threadsafe(start)
        try:
            value, error = await asyncio.shield(asyncio.wrap_future(flight.done))
        except asyncio.CancelledError as cancelled:
            self._cancel(flight)
            while not flight.done.done():
                try:
                    await asyncio.shield(asyncio.wrap_future(flight.done))
                except asyncio.CancelledError:
                    continue
            raise cancelled
        if error is not None:
            raise error
        assert value is not None
        return value

    def _cancel(self, flight: _Flight) -> None:
        def cancel() -> None:
            if flight.task is not None and not flight.task.done():
                flight.task.cancel()

        with self._lock:
            flight.cancelled = True
        self._loop.call_soon_threadsafe(cancel)

    async def quiesce(self, *, grace_seconds: float) -> None:
        """Close admission, cancel after grace, and wait for actual business cleanup."""
        with self._lock:
            self._accepting = False
            flights = tuple(self._flights)
        if not flights:
            return
        pending = {asyncio.wrap_future(flight.done) for flight in flights}
        _, pending = await asyncio.wait(pending, timeout=grace_seconds)
        if pending:
            for flight in flights:
                if not flight.done.done():
                    self._cancel(flight)
        while any(not flight.done.done() for flight in flights):
            try:
                await asyncio.gather(
                    *(asyncio.shield(asyncio.wrap_future(flight.done)) for flight in flights),
                    return_exceptions=True,
                )
            except asyncio.CancelledError:
                continue
