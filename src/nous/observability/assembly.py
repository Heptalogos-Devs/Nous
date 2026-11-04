"""Process-owned stdlib logging and OpenTelemetry assembly."""

import logging
import sys
from contextlib import ExitStack
from datetime import UTC, datetime
from logging.handlers import RotatingFileHandler
from pathlib import Path
from typing import cast

import structlog
from opentelemetry import trace
from opentelemetry.sdk.metrics import MeterProvider
from opentelemetry.sdk.metrics.export import (
    InMemoryMetricReader,
    MetricReader,
    PeriodicExportingMetricReader,
)
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import SpanLimits, TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, ConsoleSpanExporter
from opentelemetry.sdk.trace.sampling import ALWAYS_OFF, ParentBased, TraceIdRatioBased
from structlog.typing import EventDict

from nous.configuration.definitions import LoggingConfig, MetricsConfig, TracingConfig

from .instrumentation import active_providers


def _context(logger: object, method: str, event: EventDict) -> EventDict:
    current = trace.get_current_span().get_span_context()
    if current.is_valid:
        event["trace_id"] = trace.format_trace_id(current.trace_id)
        event["span_id"] = trace.format_span_id(current.span_id)
    event["timestamp"] = datetime.now(UTC).isoformat()
    event.pop("message", None)
    return event


def _terminal_event(logger: object, method: str, event: EventDict) -> EventDict:
    value = dict(event)
    for key in ("instance_id", "startup_id", "trace_id", "span_id", "attempts", "result_refs"):
        value.pop(key, None)
    if value.get("event") != "operation.failed":
        value.pop("operation_id", None)
    if value.get("event") == "request.attempt":
        route = value.pop("route", None)
        if isinstance(route, dict):
            route = cast(dict[str, object], route)
            value["model"] = route.get("model_id", route.get("model", "unknown"))
        for key in ("start", "end", "provider_request_id", "reported_usage"):
            value.pop(key, None)
    return value


class LoggingSession:
    """Replace only handlers owned by Nous; leave host handlers intact."""

    def __init__(self, startup_id: str) -> None:
        self.handlers: list[logging.Handler] = []
        self.previous_level = logging.getLogger().level
        self.tokens = structlog.contextvars.bind_contextvars(startup_id=startup_id)
        self._install("INFO", None)

    def _install(
        self, level: str, file: Path | None, max_bytes: int = 10485760, backups: int = 5
    ) -> None:
        shared: list[structlog.typing.Processor] = [
            structlog.contextvars.merge_contextvars,
            structlog.stdlib.add_log_level,
            structlog.stdlib.add_logger_name,
            structlog.stdlib.ExtraAdder(),
            _context,
        ]
        terminal = logging.StreamHandler(sys.stderr)
        terminal.setFormatter(
            structlog.stdlib.ProcessorFormatter(
                foreign_pre_chain=shared,
                processors=[
                    structlog.stdlib.ProcessorFormatter.remove_processors_meta,
                    _terminal_event,
                    structlog.dev.ConsoleRenderer(colors=False),
                ],
            )
        )
        pending: list[logging.Handler] = [terminal]
        if file is not None:
            try:
                file.parent.mkdir(parents=True, exist_ok=True)
                handler = RotatingFileHandler(
                    file, maxBytes=max_bytes, backupCount=backups, encoding="utf-8"
                )
                handler.setFormatter(
                    structlog.stdlib.ProcessorFormatter(
                        foreign_pre_chain=shared,
                        processors=[
                            structlog.processors.format_exc_info,
                            structlog.stdlib.ProcessorFormatter.remove_processors_meta,
                            structlog.processors.JSONRenderer(ensure_ascii=False),
                        ],
                    )
                )
                pending.append(handler)
            except OSError as error:
                sys.stderr.write(f"Nous log file unavailable: {error}\n")
        root = logging.getLogger()
        for handler in pending:
            root.addHandler(handler)
        for handler in self.handlers:
            root.removeHandler(handler)
            handler.close()
        self.handlers = pending
        root.setLevel(level)
        structlog.configure(
            processors=[*shared, structlog.stdlib.ProcessorFormatter.wrap_for_formatter],
            logger_factory=structlog.stdlib.LoggerFactory(),
            cache_logger_on_first_use=True,
        )
        for name in (
            "aiosqlite",
            "sqlalchemy.engine",
            "urllib3",
            "httpx",
            "httpcore",
            "httpx2",
            "httpcore2",
        ):
            logging.getLogger(name).setLevel(logging.WARNING)

    def configure(self, settings: LoggingConfig, directory: Path, instance_id: str) -> None:
        structlog.contextvars.bind_contextvars(instance_id=instance_id)
        self._install(
            str(settings.level),
            directory / "nous.jsonl" if settings.file_enabled else None,
            int(settings.max_file_bytes),
            int(settings.backup_count),
        )

    def set_level(self, level: str) -> None:
        root = logging.getLogger()
        previous = root.level
        try:
            root.setLevel(level)
        except Exception as primary:
            try:
                logging.Logger.setLevel(root, previous)
            except Exception as cleanup:
                primary.add_note(f"Restoring the previous log level also failed: {cleanup!r}")
            raise

    def close(self) -> None:
        root = logging.getLogger()
        for handler in self.handlers:
            root.removeHandler(handler)
            handler.flush()
            handler.close()
        self.handlers.clear()
        root.setLevel(self.previous_level)
        structlog.contextvars.reset_contextvars(**self.tokens)
        structlog.contextvars.unbind_contextvars("instance_id")


class Observability:
    def __init__(
        self, tracing: TracingConfig, metric_settings: MetricsConfig, instance_id: str
    ) -> None:
        with ExitStack() as initializing:
            resource = Resource.create(
                {
                    "service.name": "nous",
                    "service.version": "0.1.0",
                    "service.instance.id": instance_id,
                }
            )
            self.trace = TracerProvider(
                resource=resource,
                sampler=ParentBased(TraceIdRatioBased(float(tracing.sample_ratio)))
                if tracing.enabled
                else ALWAYS_OFF,
                span_limits=SpanLimits(max_events=64, max_attribute_length=1024),
                shutdown_on_exit=False,
            )
            initializing.callback(self.trace.shutdown)
            if tracing.enabled:
                exporter = tracing.export
                if exporter == "stderr":
                    self.trace.add_span_processor(
                        BatchSpanProcessor(ConsoleSpanExporter(out=sys.stderr))
                    )
                elif exporter == "otlp":
                    from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
                        OTLPSpanExporter,
                    )

                    self.trace.add_span_processor(
                        BatchSpanProcessor(
                            OTLPSpanExporter(
                                endpoint=str(tracing.otlp_endpoint),
                                timeout=float(tracing.export_timeout_seconds),
                            )
                        )
                    )
            self.reader = InMemoryMetricReader()
            readers: list[MetricReader] = [self.reader]
            if metric_settings.enabled and metric_settings.export == "otlp":
                from opentelemetry.exporter.otlp.proto.http.metric_exporter import (
                    OTLPMetricExporter,
                )

                readers.append(
                    PeriodicExportingMetricReader(
                        OTLPMetricExporter(
                            endpoint=str(metric_settings.otlp_endpoint),
                            timeout=float(metric_settings.export_timeout_seconds),
                        ),
                        export_interval_millis=float(metric_settings.interval_seconds) * 1000,
                    )
                )
            self.metrics = MeterProvider(
                resource=resource,
                metric_readers=readers if metric_settings.enabled else [],
                shutdown_on_exit=False,
            )
            initializing.callback(self.metrics.shutdown)
            self.timeout_millis = (
                max(
                    float(tracing.export_timeout_seconds),
                    float(metric_settings.export_timeout_seconds),
                )
                * 1000
            )
            self._token = active_providers.set(self)
            initializing.pop_all()

    def close(self) -> None:
        active_providers.reset(self._token)
        try:
            self.trace.force_flush(timeout_millis=int(self.timeout_millis))
            self.metrics.force_flush(timeout_millis=self.timeout_millis)
        except Exception:
            logging.getLogger(__name__).warning("telemetry.flush_failed", exc_info=True)
        finally:
            self.trace.shutdown()
            self.metrics.shutdown(timeout_millis=self.timeout_millis)
