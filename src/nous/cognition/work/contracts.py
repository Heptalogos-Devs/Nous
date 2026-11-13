"""Work state, revisions, and explicit records of material use."""

from datetime import datetime
from typing import Literal

from pydantic import Field, model_validator

from nous.cognition.contracts import CognitiveValue, NamespaceSelector
from nous.cognition.material_contracts import MaterialRef, MaterialSelection
from nous.content.contracts import ArtifactRef, ContentSelection, ResultRef

from .refs import WorkRef, WorkRevisionRef


class WorkDecision(CognitiveValue):
    key: str = Field(min_length=1)
    statement: str = Field(min_length=1)
    basis: tuple[MaterialSelection, ...] = ()
    state: Literal["active", "superseded", "withdrawn"] = "active"


class WorkQuestion(CognitiveValue):
    key: str = Field(min_length=1)
    question: str = Field(min_length=1)
    state: Literal["open", "resolved"] = "open"
    references: tuple[MaterialSelection, ...] = ()


class WorkState(CognitiveValue):
    title: str = Field(min_length=1)
    objective: str = Field(min_length=1)
    focus: str | None = None
    decisions: tuple[WorkDecision, ...] = ()
    questions: tuple[WorkQuestion, ...] = ()
    progress: str = ""
    blockers: tuple[str, ...] = ()
    active: tuple[MaterialSelection, ...] = ()
    related: tuple[MaterialRef, ...] = ()

    @model_validator(mode="after")
    def unique_keys(self) -> WorkState:
        for values in (self.decisions, self.questions):
            keys = [value.key for value in values]
            if len(keys) != len(set(keys)):
                raise ValueError("Work item keys must be unique within their collection")
        return self


class WorkAttributes(CognitiveValue):
    state: WorkState
    recorded_at: datetime
    fields: dict[str, ContentSelection]


class WorkView(CognitiveValue):
    ref: WorkRef
    revision_ref: WorkRevisionRef
    current_revision: WorkRevisionRef
    state: WorkState
    body_artifact: ArtifactRef
    rendered_artifact: ArtifactRef
    fields: dict[str, ContentSelection]
    body: str
    recorded_at: datetime


class CreateWork(CognitiveValue):
    state: WorkState
    body: str = ""
    namespace: NamespaceSelector = None


class ReadWork(CognitiveValue):
    ref: str | WorkRef | WorkRevisionRef
    namespace: NamespaceSelector = None


class ListWorkUses(CognitiveValue):
    ref: str | WorkRef | WorkRevisionRef
    namespace: NamespaceSelector = None
    limit: int = Field(default=20, ge=1, le=200)


class ReviseWork(CognitiveValue):
    target: str | WorkRef | WorkRevisionRef
    expected: str | int | WorkRevisionRef | None = None
    state: WorkState
    body: str
    namespace: NamespaceSelector = None


class SetActiveWork(CognitiveValue):
    target: str | WorkRef | WorkRevisionRef
    expected: str | int | WorkRevisionRef | None = None
    active: tuple[MaterialSelection, ...]
    namespace: NamespaceSelector = None


class RecordWorkUse(CognitiveValue):
    revision: WorkRevisionRef
    evidence: ResultRef
    reading: ResultRef | None = None
    selections: tuple[MaterialSelection, ...] = Field(min_length=1)
    purpose: str = Field(min_length=1)
    decision_key: str | None = None

    @model_validator(mode="after")
    def selections_are_explicit(self) -> RecordWorkUse:
        if any(not item.selections for item in self.selections):
            raise ValueError("A work use must identify the selections actually used")
        return self


class WorkUse(CognitiveValue):
    id: str
    revision: WorkRevisionRef
    evidence: ResultRef
    reading: ResultRef | None = None
    selections: tuple[MaterialSelection, ...]
    purpose: str
    decision_key: str | None = None
    recorded_at: datetime
