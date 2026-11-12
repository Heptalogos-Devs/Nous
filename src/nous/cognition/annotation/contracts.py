"""Cognitive annotation decisions and their committed effects."""

from dataclasses import dataclass
from typing import Annotated, Literal

from pydantic import Field

from nous.common.operations import OperationIssue
from nous.content.annotations import AnnotationProposal
from nous.content.contracts import ExternalInput, ResultRef

from ..concept_contracts import AnnotationRef, ConceptRef, ConceptRevisionRef, ConceptView
from ..contracts import (
    CognitiveValue,
    NamespaceRef,
    RecordRef,
    RecordRevisionRef,
    RevisionSelection,
)


class AnnotationRequest(CognitiveValue):
    """Settings for one in-memory annotation plan."""

    task: str
    candidate_limit: int
    eligibility: Literal["current_recalled", "current_any", "historical"]


class ConceptCandidate(CognitiveValue):
    key: str
    concept: ConceptView


class AnnotationPlan(CognitiveValue):
    """Exact body, candidate meanings and activation precondition for one computation."""

    target: RecordRevisionRef
    body: RevisionSelection
    task: str
    candidates: tuple[ConceptCandidate, ...]
    expected_active: AnnotationRef | None
    eligibility: Literal["current_recalled", "current_any", "historical"]


type AnnotationTarget = str | RecordRef | RecordRevisionRef
type ConceptChoice = str | ConceptRef | ConceptRevisionRef


class ProposeAnnotation(CognitiveValue):
    target: AnnotationTarget
    namespace: str | NamespaceRef | None = None
    known_concepts: dict[str, ConceptChoice] | None = None
    proposal: AnnotationProposal | ExternalInput | None = None
    task: str | None = None


class ReadAnnotation(CognitiveValue):
    ref: str | AnnotationRef
    namespace: str | NamespaceRef | None = None


class ReadAnnotationProposal(CognitiveValue):
    ref: str | ResultRef


class AdoptAnnotation(CognitiveValue):
    target: AnnotationTarget
    proposal: str | ResultRef
    namespace: str | NamespaceRef | None = None
    bindings: dict[str, ConceptChoice | None] | ExternalInput = Field(default_factory=dict)
    activate: bool = True
    activate_for_current: bool = True
    require_recall: bool = False


class AnnotateRecord(CognitiveValue):
    target: AnnotationTarget
    namespace: str | NamespaceRef | None = None
    known_concepts: dict[str, ConceptChoice] | None = None
    proposal: AnnotationProposal | ExternalInput | None = None
    proposal_result: str | ResultRef | None = None
    bindings: dict[str, ConceptChoice | None] | ExternalInput = Field(default_factory=dict)
    draft_only: bool = False
    activate_for_current: bool = True
    require_recall: bool = False
    task: str | None = None


class AnnotationNotRequested(CognitiveValue):
    state: Literal["not_requested"] = "not_requested"


class AnnotationReady(CognitiveValue):
    state: Literal["ready"] = "ready"
    annotation_ref: AnnotationRef
    proposal_ref: ResultRef


class AnnotationProposed(CognitiveValue):
    state: Literal["proposed"] = "proposed"
    proposal_ref: ResultRef


class AnnotationSuperseded(CognitiveValue):
    state: Literal["superseded"] = "superseded"
    proposal_ref: ResultRef


class AnnotationFailed(CognitiveValue):
    state: Literal["failed"] = "failed"
    issue: OperationIssue


type AnnotationOutcome = Annotated[
    AnnotationNotRequested
    | AnnotationReady
    | AnnotationProposed
    | AnnotationSuperseded
    | AnnotationFailed,
    Field(discriminator="state"),
]


class AnnotationProductContext(CognitiveValue):
    body_revision: RecordRevisionRef
    expected_active_annotation: str | None
    candidates: dict[str, ConceptRevisionRef]
    eligibility: Literal["current_recalled", "current_any", "historical"]


@dataclass(frozen=True)
class AnnotationCommit:
    """The persisted annotation and any effect on the current recall view."""

    annotation: AnnotationRef
    proposal: ResultRef
    activation: Literal["active_current", "active_historical", "inactive"]
    current_view_changed: bool
    namespace: NamespaceRef
    generation: int
    conflict: OperationIssue | None = None
