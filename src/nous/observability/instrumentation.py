"""OpenTelemetry API spans and structured root-operation/request diagnostics."""

from __future__ import annotations

import asyncio
import logging
import time
from collections.abc import Generator, Mapping
from contextlib import contextmanager
from contextvars import ContextVar
from datetime import UTC, datetime
from typing import Protocol
from uuid import uuid4

import structlog
from opentelemetry import metrics, trace
from opentelemetry.trace import StatusCode
from pydantic import JsonValue

from nous.common.errors import failure_code, failure_message
from nous.common.operations import (
    OperationDiagnostics,
    OperationFrame,
    attach_context,
    current_operation,
    exception_context,
)


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


def request_attempts(*, stage_only: bool = False) -> tuple[dict[str, JsonValue], ...]:
    frame = current_operation.get()
    if frame is None:
        return ()
    return tuple(
        dict(item)
        for item in frame.root.attempts
        if not stage_only or item.get("stage") == frame.stage
    )


@contextmanager
def operation(
    name: str,
    *,
    operation_id: str | None = None,
    diagnostics: OperationDiagnostics | None = None,
    stage: str | None = None,
    root_operation: bool | None = None,
) -> Generator[trace.Span]:
    """One root correlation survives nested stages, including disabled telemetry."""
    parent = current_operation.get()
    root = diagnostics or (
        parent.root if parent else OperationDiagnostics(operation_id or str(uuid4()), name)
    )
    is_root = (
        root_operation if root_operation is not None else parent is None or parent.root is not root
    )
    frame = OperationFrame(root, stage or name)
    token = current_operation.set(frame)
    meter = get_meter("nous.operations")
    logger = logging.getLogger("nous.operations")
    started = time.monotonic()
    result = "success"
    failure: dict[str, JsonValue] | None = None
    labels = {"operation": root.name}
    with (
        structlog.contextvars.bound_contextvars(
            operation_id=root.operation_id, operation=root.name, stage=frame.stage
        ),
        get_tracer("nous.operations").start_as_current_span(name, record_exception=False) as span,
    ):
        if is_root:
            meter.create_up_down_counter("runtime.in_flight").add(1, labels)
        logger.debug("operation.started", extra={"root_operation": is_root})
        try:
            yield span
        except BaseException as error:
            result = (
                "cancelled"
                if isinstance(error, asyncio.CancelledError)
                else "timeout"
                if isinstance(error, TimeoutError)
                else "error"
            )
            span.set_attribute("error.type", type(error).__name__)
            code = failure_code(error)
            span.set_attribute("error.code", code)
            span.set_status(StatusCode.ERROR)
            context = span.get_span_context()
            attach_context(
                error, frame, trace.format_trace_id(context.trace_id) if context.is_valid else None
            )
            failure = {
                "error_code": code,
                "error_message": failure_message(error),
                "error_type": type(error).__name__,
                "details": exception_context(error),
            }
            raise
        finally:
            elapsed = time.monotonic() - started
            if is_root:
                if result == "success":
                    result = root.outcome
                span.set_attribute("nous.outcome", result)
                if result == "partial":
                    span.set_status(StatusCode.ERROR)
                for issue in root.issues:
                    span.add_event(
                        "operation.issue",
                        {
                            "stage": issue.stage,
                            "error.code": issue.code,
                            "error.message": issue.message,
                        },
                    )
                meter.create_counter("operation.count").add(1, {**labels, "result": result})
                meter.create_histogram("operation.duration", unit="s").record(
                    elapsed, {**labels, "result": result}
                )
                meter.create_up_down_counter("runtime.in_flight").add(-1, labels)
            logger.log(
                (logging.INFO if result == "success" else logging.WARNING)
                if is_root
                else logging.DEBUG,
                "operation.terminal" if is_root else "stage.completed",
                extra={
                    "root_operation": is_root,
                    "result": result,
                    "duration_seconds": elapsed,
                    "completed_count": len(root.completed_result_refs),
                    "result_refs": list(root.completed_result_refs) if is_root else [],
                    **(
                        {
                            "issues": [issue.to_dict() for issue in root.issues],
                            "attempts": list(root.attempts),
                            "completed_result_refs": list(root.completed_result_refs),
                            **(failure or {}),
                        }
                        if is_root
                        else {}
                    ),
                },
            )
            current_operation.reset(token)


class AttemptScope:
    """Mutable report only during one actual SDK submission; it is copied on completion."""

    def __init__(self, number: int, route: Mapping[str, JsonValue], task: str | None) -> None:
        frame = current_operation.get()
        self.root = frame.root if frame else None
        self.report: dict[str, JsonValue] = {
            "number": number,
            "route": dict(route),
            "task": task,
            "stage": frame.stage if frame else None,
            "start": datetime.now(UTC).isoformat(),
            "outcome": "pending",
        }
        self.started = time.monotonic()

    def complete(
        self,
        *,
        outcome: str,
        error_code: str | None = None,
        error_type: str | None = None,
        provider_status: int | None = None,
        provider_request_id: str | None = None,
        retry_reason: str | None = None,
        next_action: str | None = None,
        reported_usage: Mapping[str, JsonValue] | None = None,
    ) -> None:
        self.report.update(
            {
                key: value
                for key, value in {
                    "outcome": outcome,
                    "error_code": error_code,
                    "error_type": error_type,
                    "provider_status": provider_status,
                    "provider_request_id": provider_request_id,
                    "retry_reason": retry_reason,
                    "next_action": next_action,
                    "reported_usage": dict(reported_usage) if reported_usage is not None else None,
                }.items()
                if value is not None
            }
        )


@contextmanager
def request_attempt(
    number: int, route: Mapping[str, JsonValue], task: str | None = None
) -> Generator[AttemptScope]:
    scope = AttemptScope(number, route, task)
    with get_tracer("nous.models").start_as_current_span(
        "model.request", record_exception=False
    ) as span:
        for key, value in route.items():
            if isinstance(value, (str, int, float, bool)):
                span.set_attribute("model." + key, value)
        span.set_attribute("request.attempt", number)
        try:
            yield scope
        except BaseException as error:
            if scope.report["outcome"] == "pending":
                scope.complete(
                    outcome="cancelled" if isinstance(error, asyncio.CancelledError) else "error",
                    error_code=str(getattr(error, "code", type(error).__name__)),
                    provider_status=getattr(error, "status_code", None),
                    provider_request_id=getattr(error, "request_id", None),
                )
            span.set_status(StatusCode.ERROR)
            raise
        finally:
            if scope.report["outcome"] == "pending":
                scope.complete(outcome="success")
            scope.report["end"] = datetime.now(UTC).isoformat()
            scope.report["duration_seconds"] = time.monotonic() - scope.started
            if scope.root is not None:
                scope.root.attempts.append(dict(scope.report))
            attributes = {
                key: value
                for key, value in scope.report.items()
                if isinstance(value, (str, int, float, bool))
            }
            usage = scope.report.get("reported_usage")
            if isinstance(usage, dict):
                for key, value in usage.items():
                    if isinstance(value, (int, float)) and not isinstance(value, bool):
                        attributes["usage." + key] = value
            span.add_event("request.attempt", attributes)
            logger = logging.getLogger("nous.models.attempts")
            logger.info("request.attempt", extra=dict(scope.report))
            labels = {
                "protocol": str(route.get("protocol", "unknown")),
                "model": str(route.get("model", route.get("model_id", "unknown"))),
                "outcome": str(scope.report["outcome"]),
            }
            meter = get_meter("nous.models")
            meter.create_counter("model.request.count").add(1, labels)
            meter.create_histogram("model.request.duration", unit="s").record(
                float(scope.report["duration_seconds"]), labels
            )
            if isinstance(usage, dict):
                for direction, names in (
                    ("input", ("input_tokens", "prompt_tokens")),
                    ("output", ("output_tokens", "completion_tokens")),
                ):
                    value = next(
                        (
                            usage[name]
                            for name in names
                            if isinstance(usage.get(name), (int, float))
                        ),
                        None,
                    )
                    if isinstance(value, (int, float)) and not isinstance(value, bool):
                        meter.create_counter("model.tokens", unit="token").add(
                            value, {**labels, "direction": direction}
                        )
