"""Cognitive annotation decisions and their committed effects."""

from dataclasses import dataclass
from typing import Literal

from nous.common.operations import OperationIssue
from nous.content.contracts import ResultRef
from nous.models.generation import FrozenLanguageTask

from ..concept_contracts import AnnotationRef, ConceptView
from ..contracts import CognitiveValue, NamespaceRef, RecordRevisionRef, RevisionSelection


class AnnotationRequest(CognitiveValue):
    """Frozen semantics for one requested annotation computation."""

    computation_key: str
    task: FrozenLanguageTask
    candidate_limit: int
    eligibility: Literal["current_recalled", "current_any", "historical"]


class ConceptCandidate(CognitiveValue):
    key: str
    concept: ConceptView


class AnnotationPlan(CognitiveValue):
    """Exact body, candidate meanings and activation precondition for one computation."""

    computation_key: str
    target: RecordRevisionRef
    body: RevisionSelection
    task: FrozenLanguageTask
    candidates: tuple[ConceptCandidate, ...]
    expected_active: AnnotationRef | None
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
