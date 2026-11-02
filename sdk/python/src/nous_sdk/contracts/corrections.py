"""Saved correction intent, visible proposals and revision-aware adoption."""

from typing import Any, Literal

from pydantic import Field, model_validator

from .cognition import CognitiveBody, CognitiveRecord, CognitiveRef
from .common import Contract, Operation
from .records import RecordsQuery


class CorrectionEdit(Contract):
    reference: CognitiveRef
    body: CognitiveBody
    reason: str = ""


class PlanCorrection(Operation):
    targets: tuple[CognitiveRef, ...] = Field(default=(), max_length=100)
    source: str | None = None
    instruction: str = Field(default="", max_length=16000)
    selection: RecordsQuery = Field(default_factory=lambda: RecordsQuery(limit=50))
    edits: tuple[CorrectionEdit, ...] = Field(default=(), max_length=100)
    supports: tuple[CognitiveRef, ...] = Field(default=(), max_length=100)


class CorrectionModelTarget(Contract):
    key: str
    record: CognitiveRecord


class CorrectionModelInput(Contract):
    instruction: str
    targets: tuple[CorrectionModelTarget, ...]
    evidence: tuple[CorrectionModelTarget, ...] = ()


class CorrectionSuggestion(Contract):
    key: str
    body: CognitiveBody
    reason: str = ""

    @model_validator(mode="before")
    @classmethod
    def unwrap_record_shaped_body(cls, data: Any) -> Any:
        """Models sometimes paste the whole CognitiveRecord into body; keep only its body."""
        if not isinstance(data, dict):
            return data
        body = data.get("body")
        if (
            isinstance(body, dict)
            and "kind" not in body
            and isinstance(body.get("body"), dict)
            and isinstance(body["body"].get("kind"), str)
        ):
            return {**data, "body": body["body"]}
        return data


class CorrectionDraft(Contract):
    explanation: str = ""
    changes: tuple[CorrectionSuggestion, ...] = ()
    questions: tuple[str, ...] = ()


class CorrectionChange(Contract):
    before: CognitiveRecord
    body: CognitiveBody
    reason: str = ""


class CorrectionSnapshot(Contract):
    before: CognitiveRecord | None = None
    after: CognitiveRecord


class CorrectionAppliedGroup(Contract):
    target: CognitiveRef
    changes: tuple[CorrectionSnapshot, ...]


class CorrectionConflict(Contract):
    expected: CognitiveRef
    current: CognitiveRef | None = None
    reason: str


class CorrectionResult(Contract):
    plan_id: str
    action: Literal["apply", "rollback"]
    records: tuple[CognitiveRecord, ...] = ()
    conflicts: tuple[CorrectionConflict, ...] = ()
    commit_seq: int


class CorrectionPlan(Contract):
    id: str
    operation_id: str
    namespace_id: str
    
    created_by: str
    created_at: str
    instruction: str
    targets: tuple[CorrectionModelTarget, ...]
    evidence: tuple[CorrectionModelTarget, ...] = ()
    changes: tuple[CorrectionChange, ...] = ()
    explanation: str = ""
    questions: tuple[str, ...] = ()
    state: Literal["generating", "ready", "applied", "rolled_back", "unavailable"]
    source_material_id: str
    source_observation_id: str
    producer_call_id: str | None = None
    applied: tuple[CorrectionAppliedGroup, ...] = ()
    results: tuple[CorrectionResult, ...] = ()
    commit_seq: int


class ApplyCorrection(Operation):
    plan_id: str


class RollbackCorrection(Operation):
    plan_id: str
    reason: str = "Rollback correction"


class CorrectionPlansQuery(Contract):
    cursor: str | None = None
    limit: int = Field(default=50, ge=1, le=1000)


class CorrectionPlansPage(Contract):
    items: tuple[CorrectionPlan, ...]
    next_cursor: str | None = None
