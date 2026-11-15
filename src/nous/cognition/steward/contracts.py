"""Requests and results of evidence-based reading arrangements."""

from typing import Literal

from pydantic import Field, model_validator

from nous.cognition.contracts import CognitiveValue, NamespaceSelector
from nous.cognition.material_contracts import MaterialSelection
from nous.content.contracts import ResultRef


class ReadingItem(CognitiveValue):
    markdown: str = Field(min_length=1)
    evidence_keys: tuple[str, ...] = ()
    role: Literal["finding", "condition", "difference", "context", "open_question"]

    @model_validator(mode="after")
    def cited(self) -> ReadingItem:
        if self.role != "open_question" and not self.evidence_keys:
            raise ValueError("A reading item must cite at least one evidence key")
        return self


class ReadingBrief(CognitiveValue):
    title: str | None = None
    items: tuple[ReadingItem, ...]


class ArrangeReading(CognitiveValue):
    evidence: str | ResultRef | None = None
    materials: tuple[str | MaterialSelection, ...] = ()
    namespace: NamespaceSelector = None
    question: str | None = None
    aspects: tuple[str, ...] | None = None
    context: tuple[dict[str, str], ...] | None = None
    purpose: str | None = None
    detail: Literal["concise", "normal", "expanded"] = "normal"
    output_language: str | None = None
    task: str | None = None


class ReadingOutcome(CognitiveValue):
    evidence_ref: ResultRef | None
    reading_ref: ResultRef
    brief: ReadingBrief
