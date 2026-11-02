"""Feedback queries pin the records actually supplied and use subsequent real contacts."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveRecord, CognitiveRef
from .common import Contract, Operation, TimePoint
from .corrections import CorrectionResult


class FeedbackPolicy(Contract):
    enabled: bool = False
    window_hours: float = Field(default=12, gt=0, le=168)
    max_messages: int = Field(default=30, ge=1, le=200)
    auto_apply_threshold: float = Field(default=0.85, ge=0, le=1)


class EnqueueFeedback(Operation):
    session_id: str = Field(min_length=1, max_length=4096)
    query: str = Field(default="", max_length=65536)
    references: tuple[CognitiveRef, ...] = Field(min_length=1, max_length=100)
    queried_at: TimePoint | None = None


class FeedbackTarget(Contract):
    key: str
    record: CognitiveRecord


class FeedbackInput(Contract):
    query: str
    targets: tuple[FeedbackTarget, ...]
    messages: tuple[FeedbackTarget, ...]


class FeedbackDecision(Contract):
    decision: Literal["confirm", "reject", "correct", "supplement", "none"]
    confidence: float = Field(ge=0, le=1)
    target_keys: tuple[str, ...] = ()
    message_keys: tuple[str, ...] = ()
    instruction: str = Field(default="", max_length=16000)
    reason: str = Field(default="", max_length=8192)


class FeedbackTask(Contract):
    id: str
    namespace_id: str
    
    caller_id: str
    session_id: str
    query: str
    references: tuple[CognitiveRef, ...]
    queried_at: TimePoint
    due_at: TimePoint
    policy: FeedbackPolicy
    state: Literal[
        "pending",
        "disabled",
        "no_feedback",
        "no_change",
        "review",
        "applied",
        "rolled_back",
        "unavailable",
        "failed",
    ]
    job_id: str | None = None
    supports: tuple[CognitiveRef, ...] = ()
    input: FeedbackInput | None = None
    decision: FeedbackDecision | None = None
    producer_call_id: str | None = None
    correction_plan_id: str | None = None
    result: CorrectionResult | None = None
    error: dict[str, str] | None = None
    created_at: str | None = None
    updated_at: str | None = None
    rollback_actor: str | None = None
    rollback_reason: str = ""
    rollback_recorded_at: str | None = None
    commit_seq: int


class FeedbackQuery(Contract):
    cursor: str | None = None
    limit: int = Field(default=50, ge=1, le=200)
    states: tuple[str, ...] = ()
    session_id: str | None = None


class FeedbackPage(Contract):
    items: tuple[FeedbackTask, ...]
    next_cursor: str | None = None


class RollbackFeedback(Operation):
    task_id: str
    reason: str = "Rollback feedback correction"


class RetryFeedback(Operation):
    task_id: str
