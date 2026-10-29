"""Official HTTP tracing plus content-free request correlation and response headers."""

import asyncio
import re
import time
from collections.abc import AsyncGenerator, Mapping
from contextlib import asynccontextmanager
from uuid import uuid4
from weakref import WeakSet

import structlog
from fastapi import FastAPI
from opentelemetry import trace
from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor
from starlette.datastructures import Headers, MutableHeaders
from starlette.routing import Route
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from nous.platform.observability.context import request_scope
from nous.platform.observability.tracing import configure_tracing

_apps: WeakSet[FastAPI] = WeakSet()
_REQUEST_ID = re.compile(
    r"(?:[0-9a-fA-F]{32}|[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12})\Z"
)


class RequestObservation:
    def __init__(self, app: ASGIApp):
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        incoming = Headers(scope=scope).get("x-nous-trace-id", "")
        selected = incoming if _REQUEST_ID.fullmatch(incoming) else str(uuid4())
        with request_scope(selected):
            await self.observe(scope, receive, send, selected)

    async def observe(self, scope: Scope, receive: Receive, send: Send, selected: str) -> None:
        started = time.monotonic()
        status = 500
        trace_fields: dict[str, str] = {}
        error_type: str | None = None

        async def observed_send(message: Message) -> None:
            nonlocal status
            if message["type"] == "http.response.start":
                status = int(message["status"])
                context = trace.get_current_span().get_span_context()
                if context.is_valid:
                    trace_fields["trace_id"] = trace.format_trace_id(context.trace_id)
                    trace_fields["span_id"] = trace.format_span_id(context.span_id)
                headers = MutableHeaders(scope=message)
                headers["x-nous-trace-id"] = trace_fields.get("trace_id", selected)
                headers["x-nous-request-id"] = selected
            await send(message)

        try:
            await self.app(scope, receive, observed_send)
        except BaseException as error:
            error_type = type(error).__name__
            raise
        finally:
            route = scope.get("route")
            fields = structlog.contextvars.get_contextvars()
            allowed = {
                key: value
                for key, value in fields.items()
                if key
                in {
                    "request_id",
                    "caller_id",
                    "namespace_id",
                    "subject_id",
                    "operation_id",
                    "job_id",
                    "source_ids",
                    "invocation_id",
                }
            }
            structlog.get_logger("nous.http").info(
                "http.request",
                method=scope["method"],
                route=route.path if isinstance(route, Route) else "unmatched",
                status=status,
                duration_ms=round((time.monotonic() - started) * 1000, 3),
                error_type=error_type,
                **allowed,
                **trace_fields,
            )


def instrument_app(app: FastAPI) -> None:
    """Call once during HTTP composition, before the app handles requests."""
    if app in _apps:
        return
    provider = configure_tracing()
    FastAPIInstrumentor.instrument_app(
        app,
        tracer_provider=provider,
        exclude_spans=["receive", "send"],
        http_capture_headers_server_request=["^$"],
        http_capture_headers_server_response=["^$"],
        http_capture_headers_sanitize_fields=[".*"],
    )
    app.add_middleware(RequestObservation)
    original = app.router.lifespan_context

    @asynccontextmanager
    async def lifespan(host: FastAPI) -> AsyncGenerator[Mapping[str, object]]:
        try:
            async with original(host) as state:
                yield state if state is not None else {}
        finally:
            flushed = await asyncio.to_thread(provider.force_flush, 5000)
            if not flushed:
                structlog.get_logger("nous.telemetry").warning("telemetry.flush_timeout")

    app.router.lifespan_context = lifespan
    _apps.add(app)
