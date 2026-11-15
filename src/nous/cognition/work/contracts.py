"""Work state, revisions, and explicit records of material use."""

from datetime import datetime
from typing import Literal

from pydantic import Field, model_validator

from nous.cognition.contracts import CognitiveValue, NamespaceSelector
from nous.cognition.material_contracts import MaterialSelection
from nous.common.material_refs import WorkRef, WorkRevisionRef
from nous.content.contracts import ArtifactRef, ContentSelection, MaterialRef, ResultRef


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


class WorkContextField(CognitiveValue):
    key: str
    selection: ContentSelection


class WorkContext(CognitiveValue):
    ref: WorkRevisionRef
    fields: tuple[WorkContextField, ...]
    active: tuple[MaterialSelection, ...]
    recent: tuple[tuple[MaterialSelection, ...], ...]


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
    body: str | None = None
    namespace: NamespaceSelector = None


class SetActiveWork(CognitiveValue):
    target: str | WorkRef | WorkRevisionRef
    expected: str | int | WorkRevisionRef | None = None
    active: tuple[MaterialSelection, ...]
    namespace: NamespaceSelector = None


class RecordWorkUse(CognitiveValue):
    revision: str | WorkRevisionRef
    evidence: str | ResultRef | None = None
    reading: str | ResultRef | None = None
    selections: tuple[MaterialSelection, ...] = ()
    evidence_keys: tuple[str, ...] = ()
    purpose: str = Field(min_length=1)
    decision_key: str | None = None
    namespace: NamespaceSelector = None

    @model_validator(mode="after")
    def selections_are_explicit(self) -> RecordWorkUse:
        if not self.selections and not self.evidence_keys:
            raise ValueError("A work use must identify the selections actually used")
        if self.evidence_keys and self.evidence is None:
            raise ValueError("Evidence keys require their saved evidence")
        if any(not item.selections for item in self.selections):
            raise ValueError("A work use must identify the selections actually used")
        return self


class WorkUse(CognitiveValue):
    id: str
    revision: WorkRevisionRef
    evidence: ResultRef | None = None
    reading: ResultRef | None = None
    selections: tuple[MaterialSelection, ...]
    purpose: str
    decision_key: str | None = None
    recorded_at: datetime
