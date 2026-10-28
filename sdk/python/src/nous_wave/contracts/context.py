"""Working contexts, actual host input admission and distinct usage observations."""

from __future__ import annotations

from datetime import datetime
from typing import Annotated, Literal

from pydantic import Field, JsonValue, model_validator

from .cognition import CognitiveRef, EpistemicStance
from .common import Contract, Operation, TimePoint
from .media import MediaReference
from .retrieval import QueryRequest


class ContextCommand(Operation):
    references: tuple[CognitiveRef, ...]
    purpose: str
    kind: Literal["active", "resident", "work", "managed"] = "work"
    executor: str = "subject"
    token_budget: int = Field(default=8192, gt=0)


class ContextFeedback(Operation):
    context_id: str
    reference: CognitiveRef
    kind: Literal["model_contact", "actual_use", "behavior_adoption", "outcome"]
    description: str
    outcome: Literal["helpful", "unhelpful", "unknown"] = "unknown"


class CognitiveTarget(Contract):
    kind: Literal["cognition"] = "cognition"
    reference: CognitiveRef


class MediaTarget(Contract):
    kind: Literal["media"] = "media"
    reference: MediaReference


ContextTarget = Annotated[CognitiveTarget | MediaTarget, Field(discriminator="kind")]


class ContextUsage(Contract):
    purpose: str = Field(min_length=1, max_length=512)
    intent: str = Field(default="", max_length=8192)
    audience: str = Field(default="subject", min_length=1, max_length=512)
    view: Literal["subject", "execution", "review", "maintenance"] = "subject"
    role: str | None = None
    activity_ref: str | None = None
    world_id: str | None = None
    world_time: TimePoint | None = None


class ContextCursor(Contract):
    track_id: str
    epoch_id: str
    revision: int = Field(ge=1)
    projection_id: str


class ContextPrepare(Operation):
    session_id: str = Field(min_length=1)
    consumer_id: str = Field(min_length=1)
    instance_id: str = Field(min_length=1)
    usage: ContextUsage
    max_items: int = Field(default=64, ge=1, le=256)
    max_text_bytes: int = Field(default=65536, ge=1, le=1048576)
    current_sources: tuple[ContextTarget, ...] = ()
    query: QueryRequest | None = None
    known_cursor: ContextCursor | None = None


class ContextSegment(Contract):
    key: str
    text: str
    sources: tuple[ContextTarget, ...]
    required: bool = False
    dependencies: tuple[ContextTarget, ...] = ()
    authority: Literal[
        "recorded_cognition", "source_material", "task_directive", "steward_interpretation"
    ]
    provenance: dict[str, JsonValue] = Field(default_factory=dict)
    epistemic: EpistemicStance | None = None


class EpistemicView(Contract):
    classifications: tuple[str, ...] = ()
    narrative_sources: tuple[str, ...] = ()
    simulated_sources: tuple[str, ...] = ()
    attribution_required: bool = True


class ContextProjection(Contract):
    id: str
    namespace_id: str
    subject_id: str
    session_id: str
    consumer_id: str
    instance_id: str
    usage: ContextUsage
    source_watermark: int
    segments: tuple[ContextSegment, ...]
    epistemic_view: EpistemicView
    cursor: ContextCursor
    config_digest: str
    recorded_at: str
    commit_seq: int
    steward_call_id: str | None = None
    degradations: tuple[str, ...] = ()


class ManagedContextResponse(Contract):
    kind: Literal["reset", "append"]
    cursor: ContextCursor
    projection: ContextProjection


class ContextPreparation(Contract):
    id: str
    state: Literal["prepared", "cancelled", "admitted"]
    request: ContextPrepare
    response: ManagedContextResponse


class HostInputMeasurement(Contract):
    input_tokens: int = Field(ge=0)
    reserved_output_tokens: int = Field(ge=0)
    context_window_tokens: int = Field(gt=0)
    source: str = Field(min_length=1, max_length=256)

    @model_validator(mode="after")
    def within_window(self) -> HostInputMeasurement:
        if self.input_tokens + self.reserved_output_tokens > self.context_window_tokens:
            raise ValueError("the complete input and reserved output exceed the context window")
        return self


class ContextAdmissionCommand(Operation):
    preparation_id: str
    invocation_id: str = Field(min_length=1)
    applied_segment_ids: tuple[str, ...]
    rendered_digest: str = Field(pattern=r"^[0-9a-f]{64}$")
    history_generation: str = Field(min_length=1)
    measurement: HostInputMeasurement


class ContextAdmission(Contract):
    id: str
    projection_id: str
    namespace_id: str
    subject_id: str
    session_id: str
    consumer_id: str
    instance_id: str
    invocation_id: str
    rendered_digest: str
    history_generation: str
    segments: tuple[str, ...]
    sources: tuple[ContextTarget, ...]
    measurement: HostInputMeasurement
    recorded_at: str
    commit_seq: int


class ContextUse(Contract):
    event_id: str = Field(min_length=1)
    target: ContextTarget
    kind: Literal[
        "referenced", "acted_on", "result_supported", "result_refuted", "corrected", "pinned"
    ]


class ContextReport(Operation):
    admission_id: str
    invocation_id: str = Field(min_length=1)
    outcome: Literal["completed", "failed", "cancelled"]
    occurred_at: datetime
    uses: tuple[ContextUse, ...] = Field(default=(), max_length=256)

    @model_validator(mode="after")
    def timestamp(self) -> ContextReport:
        if self.occurred_at.tzinfo is None:
            raise ValueError("context outcome requires a timestamp with an explicit timezone")
        if len({use.event_id for use in self.uses}) != len(self.uses):
            raise ValueError("Context use event IDs must be distinct")
        return self


class ContextReportResult(Contract):
    id: str
    admission_id: str
    invocation_id: str
    outcome: Literal["completed", "failed", "cancelled"]
    occurred_at: datetime
    uses: tuple[ContextUse, ...]
    commit_seq: int


class OpenContextSession(Operation):
    session_id: str | None = None


class ContextSession(Contract):
    id: str
    namespace_id: str
    subject_id: str
    closed: bool
    recorded_at: str
    closed_at: str | None = None
    commit_seq: int
    revision: int = Field(default=1, ge=1)
    history_generation: str | None = None
    active_work_context_id: str | None = None
    residents: tuple[ContextResident, ...] = ()


class ContextResident(Contract):
    target: ContextTarget
    state: Literal["resident", "provisional", "evicted"] = "resident"
    entry_reason: str = Field(min_length=1, max_length=2048)
    entered_at: datetime
    last_meaningful_use_at: datetime | None = None
    hold_until: datetime | None = None

    @model_validator(mode="after")
    def timestamps(self) -> ContextResident:
        if any(
            value is not None and value.tzinfo is None
            for value in (self.entered_at, self.last_meaningful_use_at, self.hold_until)
        ):
            raise ValueError("Resident timestamps require explicit timezones")
        return self


class ChangeResidents(Operation):
    session_id: str
    expected_revision: int = Field(ge=1)
    action: Literal["admit", "evict", "clear"]
    residents: tuple[ContextResident, ...] = ()
    targets: tuple[ContextTarget, ...] = ()


class WorkContextContent(Contract):
    purpose: str = Field(min_length=1, max_length=512)
    context_text: str = Field(default="", max_length=65536)
    unresolved_questions: tuple[str, ...] = ()
    constraints: JsonValue = None
    resume_conditions: tuple[str, ...] = ()
    budget_summary: JsonValue = None
    anchors: tuple[ContextTarget, ...] = ()


class WorkContext(Contract):
    id: str
    namespace_id: str
    subject_id: str
    state: Literal["open", "suspended", "ended"] = "open"
    revision: int = Field(ge=1)
    content: WorkContextContent
    recorded_at: str
    updated_at: str
    commit_seq: int


class ChangeWorkContext(Operation):
    work_context_id: str | None = None
    expected_revision: int | None = Field(default=None, ge=1)
    action: Literal[
        "create", "edit", "pause", "resume", "end", "clear_text", "clear_anchors", "pin", "unpin"
    ]
    content: WorkContextContent | None = None
    session_id: str | None = None


class ContextConsumerPolicy(Contract):
    max_items: int = Field(default=64, ge=1, le=256)
    max_text_bytes: int = Field(default=65536, ge=1, le=1048576)
    max_residents: int = Field(default=64, ge=0, le=1024)
    allowed_kinds: tuple[str, ...] = ()
    allow_media: bool = True
    steward_role: str | None = None


class ContextsPolicy(Contract):
    consumers: dict[str, ContextConsumerPolicy] = Field(
        default_factory=lambda: {
            "default": ContextConsumerPolicy(),
            "reply": ContextConsumerPolicy(),
            "maibot": ContextConsumerPolicy(),
            "mcp": ContextConsumerPolicy(),
        }
    )
    resident_limit: int = Field(default=256, ge=0, le=1024)
    resident_idle_seconds: int = Field(default=86400, ge=60)


class StewardProposal(Contract):
    selected_ids: tuple[str, ...] = Field(default=(), max_length=64)
    summary: str | None = Field(default=None, max_length=8192)

    @model_validator(mode="after")
    def selections(self) -> StewardProposal:
        if len(set(self.selected_ids)) != len(self.selected_ids):
            raise ValueError("Steward selections must be distinct")
        if self.summary and not self.selected_ids:
            raise ValueError("Steward interpretation requires selected sources")
        return self


class ContextPreparationJob(Contract):
    request_id: str
    job_id: str
