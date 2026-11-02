"""Purpose-specific requests and small language-model results for cognitive work."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveRef
from .common import Contract, Operation, TimeRange
from .retrieval import QueryRequest


class CreateJournal(Operation):
    period: TimeRange | None = None
    episodes: tuple[CognitiveRef, ...] = ()
    references: tuple[CognitiveRef, ...] = ()
    revisits: tuple[CognitiveRef, ...] = ()
    style: Literal["diary", "work_review"] = "diary"
    purpose: str = Field(default="", max_length=8192)
    limit: int = Field(default=48, ge=1, le=256)
    auto_adopt: bool = True


class ReflectRequest(Operation):
    question: str = Field(min_length=1, max_length=8192)
    references: tuple[CognitiveRef, ...] = ()
    query: QueryRequest | None = None
    limit: int = Field(default=32, ge=1, le=128)
    auto_adopt: bool = False


class DreamRequest(Operation):
    theme: str = Field(min_length=1, max_length=8192)
    references: tuple[CognitiveRef, ...] = ()
    query: QueryRequest | None = None
    limit: int = Field(default=32, ge=1, le=128)
    auto_adopt: bool = False


class SummarizeWork(Operation):
    work_context: str = Field(min_length=1)
    purpose: str = Field(default="", max_length=8192)
    limit: int = Field(default=48, ge=1, le=256)
    auto_adopt: bool = True


class JournalParagraphDraft(Contract):
    text: str = Field(min_length=1, max_length=16000)
    source_keys: tuple[str, ...] = Field(min_length=1, max_length=32)


class JournalDraft(Contract):
    title: str = Field(min_length=1, max_length=512)
    paragraphs: tuple[JournalParagraphDraft, ...] = Field(min_length=1, max_length=64)


class ReflectionDraft(Contract):
    explanation: str = Field(min_length=1)
    candidates: tuple[str, ...] = Field(
        default=(), max_length=8, description="本次明确值得形成的新认识；没有新认识时留空"
    )
    questions: tuple[str, ...] = Field(default=(), max_length=12)


class DreamDraft(Contract):
    hypotheses: tuple[str, ...] = Field(default=(), max_length=8)
    questions: tuple[str, ...] = Field(default=(), max_length=12)


class LearningPhrase(Contract):
    topic: str = Field(min_length=1, description="这条认识谈的对象、概念或方面")
    text: str = Field(min_length=1, description="资料中明确表达的这项内容")
