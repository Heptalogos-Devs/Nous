"""Three-valued desired-condition evaluation over exact bounded evidence."""

from typing import Literal

from pydantic import Field, JsonValue

from .cognition import DesiredCondition
from .cognitive_identity import CognitiveRef
from .common import Contract, Operation, TimePoint, TimeRange

MotivationSemanticStatus = Literal["off", "not_needed", "ready", "unavailable"]


class AssessMotivation(Operation):
    reference: CognitiveRef
    period: TimeRange | None = None
    at: TimePoint | None = None
    evidence: tuple[CognitiveRef, ...] = ()
    semantic: Literal["off", "optional", "required"] = "off"
    save: bool = False


class MotivationVerdict(Contract):
    key: str
    state: Literal["true", "false", "unknown"]
    confidence: float = Field(ge=0, le=1)
    reason: str


class MotivationInterpretation(Contract):
    judgments: tuple[MotivationVerdict, ...]


class MotivationJudgment(Contract):
    reference: CognitiveRef
    state: Literal["true", "false", "unknown"]
    confidence: float = Field(ge=0, le=1)
    reason: str
    producer_call_id: str | None = None


class ConditionAssessment(Contract):
    predicate: str | None = None
    operator: str
    mode: str
    state: Literal["true", "false", "unknown"]
    satisfaction: Literal["true", "false", "unknown"]
    evidence: tuple[CognitiveRef, ...] = ()
    coverage: tuple[TimeRange, ...] = ()
    gaps: tuple[TimeRange, ...] = ()
    children: tuple["ConditionAssessment", ...] = ()
    reason: str = ""
    judgments: tuple[MotivationJudgment, ...] = ()
    semantic_status: MotivationSemanticStatus = "off"
    transition_window: TimeRange | None = None
    semantic_error: dict[str, JsonValue] | None = None


class MotivationAssessment(ConditionAssessment):
    reference: CognitiveRef
    evaluated_at: TimePoint
    assessment_id: str | None = None
    saved_at: str | None = None
    commit_seq: int | None = None


class SavedMotivationAssessment(Contract):
    id: str
    namespace_id: str
    
    caller_id: str
    recorded_at: str
    commit_seq: int
    request: AssessMotivation
    condition: DesiredCondition
    description: str
    assessment: MotivationAssessment
    references: tuple[CognitiveRef, ...]
    source_ids: tuple[str, ...]


class MotivationAssessmentsQuery(Contract):
    reference: CognitiveRef | None = None
    limit: int = Field(default=50, ge=1, le=1000)
    cursor: str | None = None


class MotivationAssessmentsPage(Contract):
    items: tuple[SavedMotivationAssessment, ...]
    next_cursor: str | None = None
