"""Lightweight operation instrumentation using the OpenTelemetry API."""

import logging
import time
from collections.abc import Generator
from contextlib import contextmanager
from contextvars import ContextVar
from typing import Protocol
from uuid import uuid4

import structlog
from opentelemetry import metrics, trace
from opentelemetry.trace import StatusCode


class _Providers(Protocol):
    @property
    def trace(self) -> trace.TracerProvider: ...

    @property
    def metrics(self) -> metrics.MeterProvider: ...


active_providers: ContextVar[_Providers | None] = ContextVar("nous_observability", default=None)


def get_tracer(name: str) -> trace.Tracer:
    active = active_providers.get()
    return active.trace.get_tracer(name) if active else trace.get_tracer(name)


def get_meter(name: str) -> metrics.Meter:
    active = active_providers.get()
    return active.metrics.get_meter(name) if active else metrics.get_meter(name)


@contextmanager
def operation(name: str, *, operation_id: str | None = None) -> Generator[trace.Span]:
    """One public operation: bounded span, correlation, outcome and actual duration."""
    meter = get_meter("nous.operations")
    count = meter.create_counter("operation.count")
    duration = meter.create_histogram("operation.duration", unit="s")
    in_flight = meter.create_up_down_counter("runtime.in_flight")
    logger = logging.getLogger("nous.operations")
    started = time.monotonic()
    result = "success"
    labels = {"operation": name}
    with (
        structlog.contextvars.bound_contextvars(
            operation_id=operation_id or str(uuid4()), operation=name
        ),
        get_tracer("nous.operations").start_as_current_span(name, record_exception=False) as span,
    ):
        in_flight.add(1, labels)
        logger.info("operation.started")
        try:
            yield span
        except BaseException as error:
            import asyncio

            result = (
                "cancelled"
                if isinstance(error, asyncio.CancelledError)
                else "timeout"
                if isinstance(error, TimeoutError)
                else "error"
            )
            span.set_attribute("error.type", type(error).__name__)
            span.set_status(StatusCode.ERROR)
            raise
        finally:
            elapsed = time.monotonic() - started
            count.add(1, {**labels, "result": result})
            duration.record(elapsed, {**labels, "result": result})
            in_flight.add(-1, labels)
            logger.info(
                "operation.completed", extra={"result": result, "duration_seconds": elapsed}
            )
