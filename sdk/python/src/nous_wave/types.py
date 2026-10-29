"""Typed service receipts and records used by public SDK operations."""

from pathlib import Path
from typing import Literal

from pydantic import Field, JsonValue

from .contracts import (
    CognitiveRecord,
    CognitiveRef,
    Contract,
    MediaInfo,
    MediaReference,
    TimePoint,
)
from .contracts.bundles import BundlePlacement
from .contracts.common import ReferenceAddresses

JsonObject = dict[str, JsonValue]
QueryParam = str | int | float | bool | None


class Namespace(Contract):
    id: str
    name: str


class Subject(Contract):
    id: str
    namespace_id: str
    name: str
    capabilities: tuple[str, ...]
    commit_seq: int


class Material(Contract):
    id: str
    namespace_id: str
    content_hash: str
    text: str | None
    filename: str
    media_type: str
    byte_len: int
    source_type: str
    source: str | None = None
    origin: dict[str, str]
    event_time: TimePoint | None = None
    recorded_at: str
    commit_seq: int
    media_info: MediaInfo | None = None
    required_source_ids: tuple[str, ...] = ()
    required_observation_ids: tuple[str, ...] = ()
    addresses: ReferenceAddresses | None = None
    purged: bool = False


class MaterialReceipt(Contract):
    result: Material
    commit_seq: int


class Source(Contract):
    id: str
    namespace_id: str
    subject_id: str
    material_id: str
    kind: str
    origin: dict[str, str]
    source_type: str
    source: str | None = None
    event_time: TimePoint | None = None
    observed_at: TimePoint
    recorded_at: str
    commit_seq: int
    active: bool = True
    purged: bool = False
    placement: BundlePlacement | None = None
    previous_observation_id: str | None = None
    retry_parent_job_id: str | None = None


class Download(Contract):
    material_id: str
    reference: MediaReference | None = None
    region_id: str | None = None
    output: Path
    byte_len: int
    media_type: str | None = None
    content_hash: str | None = None


class ImportSubmission(Contract):
    request_id: str
    job_id: str
    catalog_id: str


class FileImportSubmission(Contract):
    request_id: str
    job_id: str
    catalog_id: str | None
    skipped_success_ids: tuple[str, ...] = ()


class Job(Contract):
    workflow_id: str
    status: Literal[
        "PENDING",
        "SUCCESS",
        "ERROR",
        "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
        "CANCELLED",
        "ENQUEUED",
        "DELAYED",
    ]
    name: str
    class_name: str | None = None
    config_name: str | None = None
    authenticated_user: str | None = None
    assumed_role: str | None = None
    authenticated_roles: tuple[str, ...] | None = None
    input: JsonValue = None
    output: JsonValue = None
    error: JsonValue = None
    created_at: int | None = None
    updated_at: int | None = None
    queue_name: str | None = None
    executor_id: str | None = None
    app_version: str | None = None
    workflow_timeout_ms: int | None = None
    workflow_deadline_epoch_ms: int | None = None
    deduplication_id: str | None = None
    priority: int | None = None
    queue_partition_key: str | None = None
    forked_from: str | None = None
    was_forked_from: bool = False
    parent_workflow_id: str | None = None
    dequeued_at: int | None = None
    delay_until_epoch_ms: int | None = None
    completed_at: int | None = None
    attributes: JsonObject | None = None
    schedule_name: str | None = None
    application_name: str | None = None
    app_id: str | None = None
    recovery_attempts: int | None = None


class JobPage(Contract):
    items: tuple[Job, ...]
    next_cursor: str | None = None


class JobDetail(Contract):
    job: Job


class ContextRecord(Contract):
    id: str
    kind: Literal["active", "resident", "work", "managed"]
    executor: str
    purpose: str
    references: tuple[CognitiveRef, ...]
    token_upper_bound: int
    commit_seq: int
    objects: tuple[CognitiveRecord, ...] = ()


class LocalImportFile(Contract):
    path: Path | None = None
    staged_path: Path | None = None
    filename: str | None = None
    name: str | None = None
    format: Literal["text", "json", "jsonl"] | None = None
    source: str | None = None
    file_identity: str | None = None
    size: int | None = Field(default=None, ge=0)


class ResolvedPath(Contract):
    alias: str
    relative_path: str
    resolved_path: Path
    exists: bool
    is_file: bool
    is_dir: bool


class ErrorDetail(Contract):
    code: str = "dependency_unavailable"
    message: str = "Nous request failed"
    trace_id: str | None = None
    context: JsonObject = Field(default_factory=dict)
