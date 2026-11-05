"""Exact Concept revisions and adopted annotation values."""

from datetime import datetime

from pydantic import Field

from nous.content.annotations import AnnotationSet
from nous.content.contracts import ContentSelection, ResultRef

from .contracts import CognitiveValue, RecordRevisionRef


class ConceptRef(CognitiveValue):
    instance_id: str
    namespace_id: str
    id: str
    identifier: str


class ConceptRevisionRef(CognitiveValue):
    concept_ref: ConceptRef
    revision_id: str
    ordinal: int = Field(ge=1)

    @property
    def identifier(self) -> str:
        return f"{self.concept_ref.identifier}@{self.ordinal}"


class ConceptView(CognitiveValue):
    ref: ConceptRevisionRef
    current_revision: ConceptRevisionRef
    label: str
    description: str
    aliases: tuple[str, ...] = ()
    author: str | None = None
    recorded_at: datetime


class AnnotationRef(CognitiveValue):
    instance_id: str
    namespace_id: str
    id: str
    identifier: str
    body_revision: RecordRevisionRef


class AnnotationProposalView(CognitiveValue):
    result_ref: ResultRef
    body_revision: RecordRevisionRef
    expected_active_annotation: str | None = None
    candidates: dict[str, ConceptRevisionRef] = Field(default_factory=dict)
    annotation: AnnotationSet


class BoundOccurrence(CognitiveValue):
    key: str
    concept_key: str
    blocks: tuple[str, ...]
    selections: tuple[ContentSelection, ...]


class BoundPath(CognitiveValue):
    key: str
    label: str
    occurrences: tuple[str, ...]


class AnnotationView(CognitiveValue):
    ref: AnnotationRef
    proposal_result: ResultRef
    parser_profile: str
    bindings: dict[str, ConceptView]
    occurrences: tuple[BoundOccurrence, ...]
    paths: tuple[BoundPath, ...]
    active: bool
    generation: int
