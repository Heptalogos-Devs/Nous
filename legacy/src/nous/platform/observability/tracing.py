"""Process-owned OpenTelemetry provider and official HTTP client instrumentation."""

import os
from importlib.metadata import version
from threading import Lock
from urllib.parse import urlsplit

from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.instrumentation.httpx import HTTPX2ClientInstrumentor, HTTPXClientInstrumentor
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import SpanLimits, TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.sdk.trace.sampling import ALWAYS_OFF

_provider: TracerProvider | None = None
_lock = Lock()


def configure_tracing() -> TracerProvider:
    """Initialize once; SDK shutdown/flush belongs to the hosting process."""
    global _provider
    with _lock:
        if _provider is not None:
            return _provider
        disabled = os.getenv("OTEL_SDK_DISABLED", "false").lower() == "true"
        provider = TracerProvider(
            resource=Resource.create(
                {
                    "service.name": os.getenv("OTEL_SERVICE_NAME", "nous"),
                    "service.version": version("nous-server"),
                }
            ),
            sampler=ALWAYS_OFF if disabled else None,
            # Exception events can contain provider output and validation input.
            span_limits=SpanLimits(max_events=0, max_attribute_length=512),
        )
        endpoint = os.getenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") or os.getenv(
            "OTEL_EXPORTER_OTLP_ENDPOINT"
        )
        if endpoint and not disabled:
            parsed = urlsplit(endpoint)
            if (
                parsed.scheme not in {"http", "https"}
                or not parsed.hostname
                or parsed.username is not None
            ):
                raise ValueError(
                    "The OTEL exporter endpoint must be an HTTP(S) URL without user credentials"
                )
            provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
        trace.set_tracer_provider(provider)
        # Content capture is never enabled through header-capture environment flags.
        os.environ["OTEL_INSTRUMENTATION_HTTP_CAPTURE_HEADERS_SANITIZE_FIELDS"] = ".*"
        HTTPXClientInstrumentor().instrument(tracer_provider=provider)
        HTTPX2ClientInstrumentor().instrument(tracer_provider=provider)
        _provider = provider
        return provider
