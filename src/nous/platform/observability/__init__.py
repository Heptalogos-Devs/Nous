"""Standard logging and OpenTelemetry share operation identity at request boundaries."""

import logging
import sys
import traceback
from types import TracebackType
from typing import cast

import structlog
from opentelemetry import trace
from structlog.typing import EventDict

from .context import bind_call_context, operation_span, request_id, trace_carrier
from .tracing import configure_tracing

__all__ = [
    "configure_logging",
    "configure_tracing",
    "bind_call_context",
    "operation_span",
    "request_id",
    "trace_carrier",
]


def _trace_context(logger: object, method: str, event: EventDict) -> EventDict:
    record: object = event.get("_record")
    if isinstance(record, logging.LogRecord):
        job_id: object = getattr(record, "operationUUID", None)
        if isinstance(job_id, str):
            event.setdefault("job_id", job_id)
    context = trace.get_current_span().get_span_context()
    if context.is_valid:
        event.setdefault("trace_id", trace.format_trace_id(context.trace_id))
        event.setdefault("span_id", trace.format_span_id(context.span_id))
    return event


def _safe_exception(logger: object, method: str, event: EventDict) -> EventDict:
    info: object = event.pop("exc_info", None)
    event.pop("stack_info", None)
    event.pop("exception", None)
    if info:
        exception: object = None
        frames: TracebackType | None = None
        if info is True:
            _, exception, frames = sys.exc_info()
        elif isinstance(info, BaseException):
            exception, frames = info, info.__traceback__
        elif isinstance(info, tuple):
            parts = cast(tuple[object, ...], info)
            if len(parts) == 3:
                exception = parts[1]
                frames = parts[2] if isinstance(parts[2], TracebackType) else None
        event["event"] = "unhandled.exception"
        event["error_type"] = type(exception).__name__
        if frames is not None:
            event["error_frames"] = [
                {"file": frame.filename, "line": frame.lineno, "function": frame.name}
                for frame in traceback.extract_tb(frames)[-12:]
            ]
    return event


def configure_logging(level: str) -> None:
    shared: list[structlog.typing.Processor] = [
        structlog.contextvars.merge_contextvars,
        structlog.stdlib.add_log_level,
        structlog.stdlib.add_logger_name,
        structlog.processors.TimeStamper(fmt="iso"),
        _trace_context,
    ]
    formatter = structlog.stdlib.ProcessorFormatter(
        foreign_pre_chain=shared,
        processors=[
            _safe_exception,
            structlog.stdlib.ProcessorFormatter.remove_processors_meta,
            structlog.processors.JSONRenderer(ensure_ascii=False),
        ],
    )
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(formatter)
    logging.basicConfig(handlers=[handler], level=level.upper(), force=True)
    durable = logging.getLogger("dbos")
    for existing in tuple(durable.handlers):
        durable.removeHandler(existing)
        existing.close()
    durable.addHandler(handler)
    durable.propagate = False
    for name in (
        "httpx",
        "httpx2",
        "httpcore",
        "httpcore2",
        "openai",
        "pydantic_ai",
        "uvicorn.access",
        "aiosqlite",
        "sqlalchemy.engine",
        "urllib3",
    ):
        logging.getLogger(name).setLevel(logging.WARNING)
    structlog.configure(
        processors=[*shared, structlog.stdlib.ProcessorFormatter.wrap_for_formatter],
        logger_factory=structlog.stdlib.LoggerFactory(),
        cache_logger_on_first_use=True,
    )
