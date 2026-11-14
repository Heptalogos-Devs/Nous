"""Requests and results of evidence-based reading arrangements."""

from typing import Literal

from pydantic import Field, model_validator

from nous.cognition.contracts import CognitiveValue
from nous.common.operations import OperationIssue
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


class ReadingContextRequest(CognitiveValue):
    key: str = Field(min_length=1)
    direction: Literal["before", "after"]


class ReadingBrief(CognitiveValue):
    title: str | None = None
    items: tuple[ReadingItem, ...]
    context_requests: tuple[ReadingContextRequest, ...] = ()


class ArrangeReading(CognitiveValue):
    evidence: ResultRef
    purpose: str | None = None
    detail: Literal["concise", "normal", "expanded"] = "normal"
    output_language: str | None = None
    task: str | None = None


class ReadingOutcome(CognitiveValue):
    evidence_ref: ResultRef
    reading_ref: ResultRef
    brief: ReadingBrief
    issues: tuple[OperationIssue, ...] = ()
