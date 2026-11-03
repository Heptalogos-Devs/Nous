"""Working contexts, actual host input admission and distinct usage observations."""

from __future__ import annotations

from datetime import datetime
from typing import Literal

from pydantic import Field, JsonValue, model_validator

from .cognition import CognitiveRef, EpistemicStance
from .common import Contract, TimePoint
from .common import ModelProvenance
from .history import HistorySummary
from .social import SymbolSelection
from .targets import CognitiveTarget as CognitiveTarget
from .targets import ContextTarget as ContextTarget
from .targets import MediaTarget as MediaTarget


class ContextUsage(Contract):
    purpose: str = Field(min_length=1, max_length=512)
    intent: str = Field(default="", max_length=8192)
    audience: str = Field(default="subject", min_length=1, max_length=512)
    view: Literal["subject", "execution", "review", "maintenance"] = "subject"
    role: str | None = None
    activity_ref: str | None = None
    world_id: str | None = None
    world_time: TimePoint | None = None
    audience_entities: tuple[CognitiveRef, ...] = ()
    situations: tuple[str, ...] = ()
    symbols: SymbolSelection | None = Field(default=None, exclude_if=lambda value: value is None)
    symbols_mode: Literal["understand", "express"] = Field(
        default="understand", exclude_if=lambda value: value == "understand"
    )


class ContextCursor(Contract):
    track_id: str
    epoch_id: str
    revision: int = Field(ge=1)
    projection_id: str


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
    producer: ModelProvenance | None = None


class EpistemicView(Contract):
    classifications: tuple[str, ...] = ()
    narrative_sources: tuple[str, ...] = ()
    simulated_sources: tuple[str, ...] = ()
    attribution_required: bool = True


class ContextProjection(Contract):
    id: str
    namespace_id: str

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
    steward_decision: StewardDecision | None = None
    degradations: tuple[str, ...] = ()


class ContextSession(Contract):
    id: str
    namespace_id: str

    closed: bool
    recorded_at: str
    closed_at: str | None = None
    commit_seq: int
    revision: int = Field(default=1, ge=1)
    active_work_context_id: str | None = None
    residents: tuple[ContextResident, ...] = ()
    parent_session_id: str | None = None
    history_invalidated: bool = False
    host_instances: dict[str, dict[str, JsonValue]] = Field(default_factory=dict)


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


class ChangeResidents(Contract):
    session_id: str
    expected_revision: int = Field(ge=1)
    action: Literal["admit", "evict", "clear"]
    residents: tuple[ContextResidentAdmission, ...] = ()
    targets: tuple[ContextTarget, ...] = ()


class ContextResidentAdmission(Contract):
    target: ContextTarget
    state: Literal["resident", "provisional"] = "resident"
    entry_reason: str = Field(min_length=1, max_length=2048)
    last_meaningful_use_at: datetime | None = None
    hold_until: datetime | None = None

    @model_validator(mode="after")
    def timestamps(self) -> ContextResidentAdmission:
        if any(
            value is not None and value.tzinfo is None
            for value in (self.last_meaningful_use_at, self.hold_until)
        ):
            raise ValueError("Resident admission timestamps require explicit timezones")
        return self


class WorkContextContent(Contract):
    purpose: str = Field(min_length=1, max_length=512)
    context_text: str = Field(default="", max_length=65536)
    unresolved_questions: tuple[str, ...] = ()
    constraints: JsonValue = None
    resume_conditions: tuple[str, ...] = ()
    budget_summary: JsonValue = None
    anchors: tuple[ContextTarget, ...] = ()


class WorkContext(Contract):
    purged: bool = False
    id: str
    namespace_id: str

    state: Literal["open", "suspended", "ended"] = "open"
    revision: int = Field(ge=1)
    content: WorkContextContent
    recorded_at: str
    updated_at: str
    commit_seq: int


class ChangeWorkContext(Contract):
    work_context_id: str | None = None
    expected_revision: int | None = Field(default=None, ge=1)
    action: Literal[
        "create", "edit", "pause", "resume", "end", "clear_text", "clear_anchors", "pin", "unpin"
    ]
    content: WorkContextContent | None = None
    anchors: tuple[ContextTarget, ...] = ()


class ChangeContextSession(Contract):
    session_id: str
    expected_revision: int = Field(ge=1)
    active_work_context_id: str | None = None
    close: bool = False
    invalidate_history: bool = False


class ContextConsumerPolicy(Contract):
    max_items: int = Field(default=64, ge=1, le=256)
    max_text_bytes: int = Field(default=65536, ge=1, le=1048576)
    max_residents: int = Field(default=64, ge=0, le=1024)
    allowed_kinds: tuple[str, ...] = ()
    allow_media: bool = True
    steward_role: str | None = None
    steward_max_tool_calls: int = Field(default=3, ge=0, le=16)
    steward_max_model_requests: int = Field(default=4, ge=1, le=16)
    steward_timeout_seconds: float = Field(default=30, gt=0, le=300)


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


class StewardDecision(Contract):
    selection_note: str | None = None
    group_summaries: tuple[HistorySummary, ...] = ()
    evict: tuple[ContextTarget, ...] = ()
    promote: tuple[ContextTarget, ...] = ()
    work_summary: str | None = None
    unresolved: tuple[str, ...] = ()
    sources: tuple[ContextTarget, ...] = ()
    producers: tuple[ModelProvenance, ...] = ()


class SelectionResult(Contract):
    paragraphs: list[int] = Field(
        default_factory=list, max_length=64, description="本批相关段落的编号，从1开始"
    )
    note: str | None = Field(
        default=None, max_length=512, description="关键歧义或缺少必要信息时的简短线索；其它情况留空"
    )
