"""Exact Concept revisions and adopted annotation values."""

from datetime import datetime
from typing import Literal

from pydantic import Field

from nous.content.annotations import AnnotationSet
from nous.content.contracts import ContentSelection, ResultRef

from .contracts import CognitiveValue, NamespaceRef, RecordRevisionRef


class ConceptRef(CognitiveValue):
    instance_id: str
    namespace_id: str
    id: str
    identifier: str


class ConceptRevisionRef(CognitiveValue):
    kind: Literal["concept_revision"] = "concept_revision"
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
    kind: Literal["annotation"] = "annotation"
    instance_id: str
    namespace_id: str
    id: str
    identifier: str
    body_revision: RecordRevisionRef


type ConceptTarget = str | ConceptRef | ConceptRevisionRef


class CreateConcept(CognitiveValue):
    namespace: str | NamespaceRef | None = None
    label: str
    description: str
    aliases: tuple[str, ...] = ()
    author: str | None = None


class ReadConcept(CognitiveValue):
    target: ConceptTarget
    namespace: str | NamespaceRef | None = None


class ReviseConcept(CognitiveValue):
    target: ConceptTarget
    expected_revision: ConceptRevisionRef | str
    label: str
    description: str
    aliases: tuple[str, ...] = ()
    namespace: str | NamespaceRef | None = None
    author: str | None = None


class FindConcepts(CognitiveValue):
    namespace: str | NamespaceRef | None = None
    text: str = ""
    offset: int = 0
    limit: int = 20


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
