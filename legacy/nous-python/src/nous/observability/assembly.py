"""Instance-owned logging sinks and OpenTelemetry providers."""

import logging
import sys
from collections.abc import Generator
from contextlib import ExitStack, contextmanager
from datetime import UTC, datetime
from logging.handlers import RotatingFileHandler
from pathlib import Path

import structlog
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

from .instrumentation import active_logging, active_providers, get_logger


def _context(logger: object, method: str, event: EventDict) -> EventDict:
    event["timestamp"] = datetime.now(UTC).isoformat()
    event.pop("message", None)
    return event


class LoggingSession:
    """Own one runtime's sinks without modifying process logging configuration."""

    def __init__(self, startup_id: str) -> None:
        self.handlers: list[logging.Handler] = []
        self.startup_id = startup_id
        self.route = logging.getLogger(f"nous.instances.{startup_id}")
        self.route.propagate = False
        self._install("INFO", None)

    def _install(
        self, level: str, file: Path | None, max_bytes: int = 10485760, backups: int = 5
    ) -> None:
        shared: list[structlog.typing.Processor] = [
            structlog.stdlib.add_log_level,
            structlog.stdlib.add_logger_name,
            structlog.stdlib.ExtraAdder(),
            _context,
        ]
        pending: list[logging.Handler] = [logging.NullHandler()]
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
        for handler in pending:
            self.route.addHandler(handler)
        for handler in self.handlers:
            self.route.removeHandler(handler)
            handler.close()
        self.handlers = pending
        self.route.setLevel(level)

    def logger(self, name: str) -> logging.Logger:
        return self.route.getChild(name)

    def configure(self, settings: LoggingConfig, directory: Path, instance_id: str) -> None:
        self._install(
            str(settings.level),
            directory / "nous.jsonl" if settings.file_enabled else None,
            int(settings.max_file_bytes),
            int(settings.backup_count),
        )

    def set_level(self, level: str) -> None:
        previous = self.route.level
        try:
            self.route.setLevel(level)
        except Exception as primary:
            try:
                logging.Logger.setLevel(self.route, previous)
            except Exception as cleanup:
                primary.add_note(f"Restoring the previous log level also failed: {cleanup!r}")
            raise

    def close(self) -> None:
        for handler in self.handlers:
            self.route.removeHandler(handler)
            handler.flush()
            handler.close()
        self.handlers.clear()


class Observability:
    def __init__(
        self,
        tracing: TracingConfig,
        metric_settings: MetricsConfig,
        instance_id: str,
        logging_session: LoggingSession | None = None,
    ) -> None:
        self.logging = logging_session
        self.instance_id = instance_id
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
            initializing.pop_all()

    @contextmanager
    def scope(self, instance_id: str, startup_id: str) -> Generator[None]:
        """Bind this runtime's providers even in tasks created before it opened."""
        token = active_providers.set(self)
        logging_token = active_logging.set(self.logging)
        try:
            with structlog.contextvars.bound_contextvars(
                instance_id=instance_id, startup_id=startup_id
            ):
                yield
        finally:
            active_logging.reset(logging_token)
            active_providers.reset(token)

    def close(self) -> None:
        with self.scope(self.instance_id, self.logging.startup_id if self.logging else ""):
            try:
                self.trace.force_flush(timeout_millis=int(self.timeout_millis))
                self.metrics.force_flush(timeout_millis=self.timeout_millis)
            except Exception:
                get_logger(__name__).warning("telemetry.flush_failed", exc_info=True)
            finally:
                self.trace.shutdown()
                self.metrics.shutdown(timeout_millis=self.timeout_millis)
