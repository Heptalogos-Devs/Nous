"""Correlate trusted operation identities without collecting operation content."""

import re
import time
from collections.abc import Generator, Mapping, Sequence
from contextlib import contextmanager
from contextvars import ContextVar

import structlog
from nous_wave.contracts import CallContext
from opentelemetry import propagate, trace
from opentelemetry.trace import Span, StatusCode

from .tracing import configure_tracing

_request_id: ContextVar[str | None] = ContextVar("nous_request_id", default=None)
_SAFE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:@/+\-]{0,255}\Z")


def request_id() -> str | None:
    return _request_id.get()


@contextmanager
def request_scope(value: str) -> Generator[None]:
    previous = structlog.contextvars.get_contextvars()
    structlog.contextvars.clear_contextvars()
    structlog.contextvars.bind_contextvars(request_id=value)
    token = _request_id.set(value)
    try:
        yield
    finally:
        _request_id.reset(token)
        structlog.contextvars.clear_contextvars()
        structlog.contextvars.bind_contextvars(**previous)


def correlation(
    ctx: CallContext,
    *,
    operation_id: str | None = None,
    job_id: str | None = None,
    source_ids: Sequence[str] = (),
    invocation_id: str | None = None,
) -> dict[str, str | tuple[str, ...]]:
    fields = {
        "caller_id": ctx.caller_id,
        "namespace_id": ctx.namespace_id,
        "subject_id": ctx.subject_id,
        "request_id": request_id() or ctx.request_id,
        "operation_id": operation_id,
        "job_id": job_id,
        "invocation_id": invocation_id,
    }
    result: dict[str, str | tuple[str, ...]] = {
        key: value
        for key, value in fields.items()
        if value is not None and _SAFE_ID.fullmatch(value)
    }
    if source_ids:
        result["source_ids"] = tuple(
            value for value in source_ids[:64] if _SAFE_ID.fullmatch(value)
        )
    return result


def bind_call_context(
    ctx: CallContext,
    *,
    operation_id: str | None = None,
    job_id: str | None = None,
    source_ids: Sequence[str] = (),
    invocation_id: str | None = None,
) -> None:
    """Bind only trusted identities after authentication and source-scope resolution."""
    fields = correlation(
        ctx,
        operation_id=operation_id,
        job_id=job_id,
        source_ids=source_ids,
        invocation_id=invocation_id,
    )
    structlog.contextvars.bind_contextvars(**fields)
    span = trace.get_current_span()
    for key, value in fields.items():
        span.set_attribute("nous." + key, value)


def trace_carrier() -> dict[str, str]:
    carrier: dict[str, str] = {}
    propagate.inject(carrier)
    return {key: value for key, value in carrier.items() if key in {"traceparent", "tracestate"}}


@contextmanager
def operation_span(
    name: str,
    ctx: CallContext,
    *,
    operation_id: str | None = None,
    job_id: str | None = None,
    source_ids: Sequence[str] = (),
    invocation_id: str | None = None,
    carrier: Mapping[str, str] | None = None,
) -> Generator[Span]:
    """Use at the owner of a use case or transaction; restore context on every exit."""
    provider = configure_tracing()
    fields = correlation(
        ctx,
        operation_id=operation_id,
        job_id=job_id,
        source_ids=source_ids,
        invocation_id=invocation_id,
    )
    tracer = trace.get_tracer("nous.operations", tracer_provider=provider)
    parent = (
        propagate.extract(
            {key: value for key, value in carrier.items() if key in {"traceparent", "tracestate"}}
        )
        if carrier
        else None
    )
    started = time.monotonic()
    with (
        structlog.contextvars.bound_contextvars(**fields),
        tracer.start_as_current_span(
            name,
            context=parent,
            attributes={"nous." + key: value for key, value in fields.items()},
            record_exception=False,
            set_status_on_exception=False,
        ) as span,
    ):
        try:
            yield span
        except BaseException as error:
            span.set_attribute("error.type", type(error).__name__)
            span.set_status(StatusCode.ERROR)
            structlog.get_logger("nous.operations").warning(
                "operation.failed",
                operation_kind=name,
                error_type=type(error).__name__,
                duration_ms=round((time.monotonic() - started) * 1000, 3),
            )
            raise
        else:
            structlog.get_logger("nous.operations").info(
                "operation.completed",
                operation_kind=name,
                duration_ms=round((time.monotonic() - started) * 1000, 3),
            )
