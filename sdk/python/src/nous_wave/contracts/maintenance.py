"""Reviewable, durable cognitive growth without a second scheduler state machine."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveKind, CognitiveRef
from .common import Contract, Operation
from .growth import GrowthDraft


class MaintenanceOptions(Contract):
    instruction: str = Field(default="", max_length=16000)
    kinds: tuple[CognitiveKind, ...] | None = None
    limit: int = Field(default=500, ge=1, le=1000)


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


class ReviewMaintenance(Operation):
    expected_revision: int = Field(ge=1)
    decision: Literal["adopt", "reject"]
    reason: str = Field(default="", max_length=4096)


class MaintenanceReview(Contract):
    decision: Literal["adopt", "reject"]
    reason: str
    caller_id: str
    recorded_at: str
    commit_seq: int
    adopted: tuple[CognitiveRef, ...] = ()


class MaintenanceRun(Contract):
    id: str
    namespace_id: str
    subject_id: str
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
    purged_at: str | None = None
    sources_available: bool = True


class CreateMaintenanceSchedule(Operation):
    name: str = Field(min_length=1, max_length=120)
    cron: str = Field(min_length=1, max_length=120)
    timezone: str = Field(default="UTC", min_length=1, max_length=120)
    command: MaintenanceOptions = Field(default_factory=MaintenanceOptions)


class MaintenanceSchedule(Contract):
    id: str
    namespace_id: str
    subject_id: str
    created_by: str
    created_at: str
    name: str
    cron: str
    timezone: str
    command: MaintenanceOptions
    stopped_at: str | None = None
