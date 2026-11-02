"""Reviewable, durable cognitive growth without a second scheduler state machine."""

from typing import Literal

from pydantic import Field, JsonValue

from .cognition import CognitiveKind, CognitiveRef
from .common import Contract, Operation, TimeRange
from .growth import GrowthDraft, GrowthSource
from .media import MediaReference
from .retrieval import QueryRequest


class MaintenanceOptions(Contract):
    instruction: str = Field(default="", max_length=16000)
    kinds: tuple[CognitiveKind, ...] | None = None
    limit: int = Field(default=500, ge=1, le=1000)
    references: tuple[CognitiveRef, ...] = ()
    auto_adopt: bool = False
    kind: Literal["growth", "seed", "journal", "reflect", "dream"] = "growth"
    query: QueryRequest | None = None
    period: TimeRange | None = None
    style: Literal["diary", "work_review"] = "diary"
    revisits: tuple[CognitiveRef, ...] = ()
    previous_seed_id: str | None = None
    source_catalog_id: str | None = None
    material_sources: tuple[MediaReference, ...] = ()


class StartMaintenance(MaintenanceOptions, Operation):
    pass


class MaintenanceSubmission(Contract):
    run_id: str
    job_id: str


class SnapshotReference(Contract):
    key: str
    reference: CognitiveRef


class MaintenanceCatalog(Contract):
    watermark: int
    entries: tuple[SnapshotReference, ...]
    material_ids: tuple[str, ...]
    observation_ids: tuple[str, ...]
    sources: tuple[GrowthSource, ...] = ()


class ReviewMaintenance(Operation):
    expected_revision: int = Field(ge=1)
    decision: Literal["adopt", "reject"]
    reason: str = Field(default="", max_length=4096)
    group_keys: tuple[str, ...] | None = None


class MaintenanceReview(Contract):
    decision: Literal["adopt", "reject"]
    reason: str
    caller_id: str
    recorded_at: str
    commit_seq: int
    adopted: tuple[CognitiveRef, ...] = ()


class MaintenanceGroup(Contract):
    key: str
    draft: GrowthDraft | None = None
    raw_actions: tuple[JsonValue, ...] = ()
    action_keys: tuple[str, ...] = ()
    dependencies: tuple[str, ...] = ()
    bindings: tuple[SnapshotReference, ...] = ()
    status: Literal["ready", "failed", "blocked", "adopted"] = "ready"
    error: dict[str, JsonValue] | None = None
    adopted: tuple[CognitiveRef, ...] = ()


class MaintenanceProposal(Contract):
    summary: str | None = None
    rationale: str | None = None
    actions: tuple[JsonValue, ...] = ()
    origin: Literal["model", "human"] = "model"
    complete: bool = True
    source_call_id: str | None = None
    validation_errors: tuple[dict[str, JsonValue], ...] = ()
    questions: tuple[str, ...] = ()


class CaptureMaintenanceOutput(Operation):
    expected_revision: int = Field(ge=1)
    call_id: str


class EditMaintenanceProposal(Operation):
    expected_revision: int = Field(ge=1)
    proposal: MaintenanceProposal
    reason: str = Field(min_length=1, max_length=4096)


class MaintenanceEdit(Contract):
    caller_id: str
    recorded_at: str
    commit_seq: int
    reason: str
    source_call_id: str | None = None


class MaintenanceRun(Contract):
    status: Literal["queued", "waiting", "proposed", "adopted", "rejected", "unavailable"] = (
        "queued"
    )
    id: str
    namespace_id: str

    created_by: str
    created_at: str
    job_id: str
    revision: int = 1
    command: StartMaintenance
    catalog: MaintenanceCatalog | None = None
    draft: GrowthDraft | None = None
    producer_call_id: str | None = None
    generation_call_ids: tuple[str, ...] = ()
    review: MaintenanceReview | None = None
    review_history: tuple[MaintenanceReview, ...] = ()
    purged_at: str | None = None
    sources_available: bool = True
    waiting_reason: dict[str, JsonValue] | None = None
    questions: tuple[str, ...] = ()
    groups: tuple[MaintenanceGroup, ...] = ()
    proposal: MaintenanceProposal | None = None
    edits: tuple[MaintenanceEdit, ...] = ()


class GrowthPolicy(Contract):
    mode: Literal["off", "automatic", "review"] = "off"
    max_snapshot_entries: int = Field(default=128, ge=1, le=1000)


class CreateMaintenanceSchedule(Operation):
    name: str = Field(min_length=1, max_length=120)
    cron: str = Field(min_length=1, max_length=120)
    timezone: str = Field(default="UTC", min_length=1, max_length=120)
    command: MaintenanceOptions = Field(default_factory=MaintenanceOptions)


class MaintenanceSchedule(Contract):
    id: str
    namespace_id: str

    created_by: str
    created_at: str
    name: str
    cron: str
    timezone: str
    command: MaintenanceOptions
    stopped_at: str | None = None
    last_input_digest: str | None = None
    last_run_id: str | None = None
