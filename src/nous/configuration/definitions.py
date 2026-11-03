"""Authoritative configuration definitions and cross-resource constraints."""

from fractions import Fraction
from pathlib import PurePath
from typing import Annotated, Literal, Self
from uuid import UUID
from zoneinfo import ZoneInfo

from pydantic import Field, field_validator, model_validator

from nous.models.configuration import (
    CredentialReference,
    EmbeddingConfig,
    ModelConfig,
    ModelExecutionConfig,
    PresetConfig,
    PromptConfig,
    ProviderConfig,
    TaskAssignment,
)
from nous.runtime.values import ResourceId

from .metadata import ConfigModel, setting


class InstanceConfig(ConfigModel):
    id: Annotated[
        str,
        setting(
            description="Stable instance UUID.",
            level="internal",
            apply="new_binding",
            readonly=True,
        ),
    ]
    name: str = setting(
        "default", description="Instance display name.", level="standard", min_length=1
    )
    layout: Annotated[
        Literal["platform", "portable"],
        setting(description="Instance directory layout.", apply="new_binding", readonly=True),
    ]

    @field_validator("id")
    @classmethod
    def valid_id(cls, value: str) -> str:
        UUID(value)
        return value


class PathsConfig(ConfigModel):
    data_dir: str | None = setting(
        None,
        description="Formal database root; existing data requires explicit relocation.",
        apply="new_binding",
        impact="data_bound",
    )
    content_dir: str | None = setting(
        None,
        description="Formal content root; changes require explicit relocation.",
        apply="new_binding",
        impact="data_bound",
    )
    index_dir: str | None = setting(
        None,
        description="Index root; changes require explicit relocation.",
        apply="new_binding",
        impact="data_bound",
    )
    cache_dir: str | None = setting(
        None, description="Disposable cache directory.", apply="restart"
    )
    logs_dir: str | None = setting(None, description="Log directory.", apply="restart")
    temp_dir: str | None = setting(
        None, description="Operation temporary directory.", apply="restart"
    )
    runtime_dir: str | None = setting(
        None,
        description="Runtime temporary directory; lock ownership stays with the instance.",
        apply="restart",
    )
    exports_dir: str | None = setting(None, description="User export directory.", apply="restart")


class RuntimeConfig(ConfigModel):
    shutdown_grace_seconds: float = setting(
        30, ge=0, description="Grace period for shutdown in seconds."
    )


class ExecutionConfig(ConfigModel):
    max_in_flight: int = setting(
        16, ge=1, apply="restart", description="Maximum concurrent operations."
    )
    max_blocking_workers: int = setting(
        4, ge=1, apply="restart", description="Maximum blocking worker threads."
    )
    default_timeout_seconds: float | None = setting(
        None,
        gt=0,
        description="Optional operation deadline in seconds.",
        scopes=("instance", "namespace", "task", "call"),
    )


class LoggingConfig(ConfigModel):
    level: Literal["DEBUG", "INFO", "WARNING", "ERROR"] = setting(
        "INFO", level="standard", apply="live", description="Minimum emitted log severity."
    )
    file_enabled: bool = setting(
        True, apply="restart", description="Write rotating instance log files."
    )
    max_file_bytes: int = setting(
        10485760, ge=1024, apply="restart", description="Maximum log file size in bytes."
    )
    backup_count: int = setting(
        5, ge=1, apply="restart", description="Number of rotated log files."
    )


class TracingConfig(ConfigModel):
    enabled: bool = setting(True, apply="restart", description="Enable local trace recording.")
    sample_ratio: float = setting(
        1.0, ge=0, le=1, apply="restart", description="Trace sampling ratio."
    )
    export: Literal["none", "stderr", "otlp"] = setting(
        "none", apply="restart", description="Trace export destination."
    )
    otlp_endpoint: str | None = setting(
        None,
        apply="restart",
        description="Explicit OTLP HTTP endpoint.",
        pattern=r"^https?://[^\s]+$",
    )
    export_timeout_seconds: float = setting(
        5, gt=0, level="developer", apply="restart", description="Trace export timeout in seconds."
    )

    @model_validator(mode="after")
    def endpoint_required(self) -> Self:
        if self.export == "otlp" and not self.otlp_endpoint:
            raise ValueError("tracing.otlp_endpoint is required for OTLP export")
        return self


class MetricsConfig(ConfigModel):
    enabled: bool = setting(True, apply="restart", description="Enable operation metrics.")
    export: Literal["none", "otlp"] = setting(
        "none", apply="restart", description="Metrics export destination."
    )
    interval_seconds: float = setting(
        60,
        gt=0,
        level="developer",
        apply="restart",
        description="Metric export interval in seconds.",
    )
    otlp_endpoint: str | None = setting(
        None,
        apply="restart",
        description="Explicit metrics OTLP HTTP endpoint.",
        pattern=r"^https?://[^\s]+$",
    )
    export_timeout_seconds: float = setting(
        5, gt=0, level="developer", apply="restart", description="Metric export timeout in seconds."
    )

    @model_validator(mode="after")
    def endpoint_required(self) -> Self:
        if self.export == "otlp" and not self.otlp_endpoint:
            raise ValueError("metrics.otlp_endpoint is required for OTLP export")
        return self


class LocalizationConfig(ConfigModel):
    language: Literal["zh-CN", "en"] = setting(
        "zh-CN",
        level="standard",
        description="Presentation language.",
        scopes=("instance", "namespace", "task", "call"),
    )
    timezone: str = setting(
        "UTC",
        level="standard",
        description="IANA display timezone.",
        scopes=("instance", "namespace", "task", "call"),
    )

    @field_validator("timezone")
    @classmethod
    def valid_zone(cls, value: str) -> str:
        try:
            ZoneInfo(value)
        except KeyError as error:
            raise ValueError(f"Unknown IANA timezone: {value}") from error
        return value


class DatabaseConfig(ConfigModel):
    filename: str = setting(
        "nous.sqlite3",
        apply="new_binding",
        impact="data_bound",
        description="Database file relative to the data root.",
    )
    busy_timeout_ms: int = setting(
        5000, ge=0, apply="restart", description="SQLite busy timeout in milliseconds."
    )
    synchronous: Literal["FULL", "NORMAL"] = setting(
        "FULL",
        level="developer",
        apply="restart",
        impact="disruptive",
        description="SQLite durability policy.",
    )
    foreign_keys: Literal[True] = setting(
        True,
        readonly=True,
        level="internal",
        description="SQLite foreign key enforcement is required.",
    )
    journal_mode: Literal["WAL"] = setting(
        "WAL", readonly=True, level="internal", description="SQLite uses write-ahead logging."
    )

    @field_validator("filename")
    @classmethod
    def relative_filename(cls, value: str) -> str:
        if (
            not value
            or PurePath(value).name != value
            or "/" in value
            or "\\" in value
            or value in {".", ".."}
            or ":" in value
        ):
            raise ValueError("database.filename must be a relative filename")
        return value


class TimeDomainConfig(ConfigModel):
    kind: Annotated[
        Literal["utc", "linear", "opaque"],
        setting(
            apply="new_binding",
            impact="data_bound",
            description="Coordinate interpretation; registered IDs cannot change kind.",
        ),
    ]
    tick_seconds: str | None = setting(
        None,
        apply="new_binding",
        impact="data_bound",
        description="Positive rational seconds per tick, e.g. 1/1000.",
    )
    epoch: str | None = setting(
        None, apply="new_binding", impact="data_bound", description="Coordinate origin description."
    )
    name: str = setting("", level="standard", description="Human-readable domain name.")

    @model_validator(mode="after")
    def valid_scale(self) -> Self:
        if self.kind in {"linear", "utc"} and self.tick_seconds is None:
            raise ValueError("linear/utc time domains require tick_seconds")
        if self.tick_seconds is not None:
            try:
                value = Fraction(self.tick_seconds)
                if value <= 0:
                    raise ValueError("tick_seconds must be positive")
            except (ValueError, ZeroDivisionError) as error:
                raise ValueError("tick_seconds must be a positive rational string") from error
        return self


class TimeConfig(ConfigModel):
    domains: dict[ResourceId, TimeDomainConfig] = Field(
        default_factory=lambda: {
            "utc": TimeDomainConfig(
                kind="utc", tick_seconds="1/1000000", epoch="Unix epoch", name="UTC"
            )
        }
    )


class NetworkConfig(ConfigModel):
    proxy_url: str | None = setting(
        None,
        apply="restart",
        description="Explicit HTTP or SOCKS proxy.",
        pattern=r"^(https?|socks5h?)://[^\s]+$",
    )
    trust_env: bool = setting(
        False, apply="restart", description="Allow HTTP transport environment settings."
    )
    ca_file: str | None = setting(
        None, apply="restart", description="Optional CA certificate bundle path."
    )
    connect_timeout_seconds: float = setting(10, gt=0, description="Connection timeout in seconds.")
    read_timeout_seconds: float = setting(120, gt=0, description="Read timeout in seconds.")
    write_timeout_seconds: float = setting(30, gt=0, description="Write timeout in seconds.")
    pool_timeout_seconds: float = setting(
        10, gt=0, description="Connection pool timeout in seconds."
    )
    max_connections: int = setting(
        20, ge=1, apply="restart", description="Connection pool ceiling."
    )


class NousConfig(ConfigModel):
    instance: InstanceConfig
    paths: PathsConfig = Field(default_factory=PathsConfig)
    runtime: RuntimeConfig = Field(default_factory=RuntimeConfig)
    execution: ExecutionConfig = Field(default_factory=ExecutionConfig)
    logging: LoggingConfig = Field(default_factory=LoggingConfig)
    tracing: TracingConfig = Field(default_factory=TracingConfig)
    metrics: MetricsConfig = Field(default_factory=MetricsConfig)
    localization: LocalizationConfig = Field(default_factory=LocalizationConfig)
    database: DatabaseConfig = Field(default_factory=DatabaseConfig)
    time: TimeConfig = Field(default_factory=TimeConfig)
    network: NetworkConfig = Field(default_factory=NetworkConfig)
    credentials: dict[ResourceId, CredentialReference] = Field(default_factory=dict)
    providers: dict[ResourceId, ProviderConfig] = Field(default_factory=dict)
    models: dict[ResourceId, ModelConfig] = Field(default_factory=dict)
    presets: dict[ResourceId, PresetConfig] = Field(default_factory=dict)
    tasks: dict[ResourceId, TaskAssignment] = Field(default_factory=dict)
    model_execution: ModelExecutionConfig = Field(default_factory=ModelExecutionConfig)
    prompt: PromptConfig = Field(default_factory=PromptConfig)
    embedding: EmbeddingConfig = Field(default_factory=EmbeddingConfig)

    @model_validator(mode="after")
    def references_exist(self) -> Self:
        for key, provider in self.providers.items():
            if provider.credential_ref and provider.credential_ref not in self.credentials:
                raise ValueError(
                    f"providers.{key}.credential_ref: unknown credential {provider.credential_ref}"
                )
        for key, model in self.models.items():
            if model.provider not in self.providers:
                raise ValueError(f"models.{key}.provider: unknown provider {model.provider}")
        for key, task in self.tasks.items():
            for choice in [task, *task.fallback]:
                if choice.model not in self.models:
                    raise ValueError(f"tasks.{key}: unknown model {choice.model}")
                if choice.preset and choice.preset not in self.presets:
                    raise ValueError(f"tasks.{key}: unknown preset {choice.preset}")
            for limit in ("max_attempts", "max_input_tokens", "max_output_tokens"):
                value = getattr(task.budget, limit)
                if value is not None and value > getattr(self.model_execution, limit):
                    raise ValueError(f"tasks.{key}.budget.{limit}: exceeds instance ceiling")
        for key, space in self.embedding.spaces.items():
            if space.model not in self.models:
                raise ValueError(f"embedding.spaces.{key}.model: unknown model {space.model}")
            if "embedding" not in self.models[space.model].capabilities:
                raise ValueError(f"embedding.spaces.{key}: model must declare embedding capability")
            if space.encoding.input_combination == "text" and space.modalities != ["text"]:
                raise ValueError(
                    f"embedding.spaces.{key}: text encoding requires only text modality"
                )
        utc = self.time.domains.get("utc")
        if (
            utc is None
            or utc.kind != "utc"
            or Fraction(utc.tick_seconds or "0") != Fraction(1, 1000000)
            or utc.epoch != "Unix epoch"
        ):
            raise ValueError("time.domains.utc has the fixed Unix epoch microsecond definition")
        if any(key != "utc" and item.kind == "utc" for key, item in self.time.domains.items()):
            raise ValueError("Only the built-in utc ID may use kind=utc")
        return self
