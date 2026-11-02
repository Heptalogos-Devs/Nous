"""Social account correspondence and directional judgments retain their own evidence."""

from typing import Literal

from pydantic import Field, field_validator

from .cognition import (
    CognitiveRecord,
    CognitiveRef,
    EvidenceLink,
    PlatformIdentity,
    SocialRelationBody,
)
from .common import CognitiveTimes, Contract, Operation, TimePoint
from .common import ModelProvenance
from .media import MediaReference
from .targets import ContextTarget


class LinkIdentity(Operation):
    entity: CognitiveRef
    identity: PlatformIdentity
    times: CognitiveTimes
    replace: bool = False


class ChangeSocialRelation(Operation):
    body: SocialRelationBody
    evidence: tuple[EvidenceLink, ...] = Field(min_length=1)
    times: CognitiveTimes
    target: CognitiveRef | None = None


class IdentityHypothesis(Operation):
    left: CognitiveRef
    right: CognitiveRef
    explanation: str = Field(min_length=1)
    confidence: float | None = Field(default=None, ge=0, le=1)
    evidence: tuple[EvidenceLink, ...] = Field(min_length=1)
    times: CognitiveTimes


class UnlinkIdentity(Operation):
    entity: CognitiveRef
    platform: str
    account_id: str
    reason: str = Field(min_length=1)
    evidence: tuple[EvidenceLink, ...] = Field(min_length=1)
    times: CognitiveTimes


class SymbolSelection(Contract):
    expression: str = ""
    carriers: tuple[MediaReference, ...] = ()
    intent: str = ""
    speaker: CognitiveRef | None = None
    audience: tuple[CognitiveRef, ...] = ()
    groups: tuple[CognitiveRef, ...] = ()
    contexts: tuple[str, ...] = ()
    allow_media: bool = True
    limit: int = Field(default=20, ge=1, le=100)
    targets: tuple[CognitiveRef, ...] = Field(
        default=(), max_length=100, exclude_if=lambda value: not value
    )
    messages: tuple[CognitiveRef, ...] = Field(
        default=(), max_length=30, exclude_if=lambda value: not value
    )
    message_sources: tuple[MediaReference, ...] = Field(
        default=(), max_length=30, exclude_if=lambda value: not value
    )
    at: TimePoint | None = Field(default=None, exclude_if=lambda value: value is None)
    semantic: Literal["off", "optional", "required"] = Field(
        default="off", exclude_if=lambda value: value == "off"
    )
class SymbolEvidence(Contract):
    key: str
    kind: Literal["input", "cognition", "media"]
    reference: ContextTarget | None = None


class SymbolMatch(Contract):
    reference: CognitiveRef
    basis: Literal["exact_expression", "exact_carrier", "intent", "semantic"]
    score: float | None = Field(default=None, ge=0, le=1, allow_inf_nan=False)
    confidence: float | None = Field(default=None, ge=0, le=1, allow_inf_nan=False)
    meaning: str
    guidance: str
    sources: tuple[SymbolEvidence, ...]


class SymbolInterpretation(Contract):
    content_description: str
    meaning: str
    expression: str = ""
    emotions: tuple[str, ...] = ()
    intentions: tuple[str, ...] = ()
    confidence: float | None = Field(default=None, ge=0, le=1, allow_inf_nan=False)
    guidance: str = ""
    alternatives: tuple[str, ...] = ()
    unknown_aspects: tuple[str, ...] = ()
    sources: tuple[SymbolEvidence, ...]
    attribution: Literal["source_expression", "specified_speaker"] = "source_expression"
    speaker: CognitiveRef | None = None


class SymbolCandidates(Contract):
    candidates: tuple[CognitiveRecord, ...]
    ambiguous: bool
    unknown: bool
    matches: tuple[SymbolMatch, ...] = ()
    local_interpretation: SymbolInterpretation | None = None
    semantic_status: Literal["off", "used", "unavailable"] = "off"
    producer: ModelProvenance | None = None
    dependencies: tuple[ContextTarget, ...] = ()
    at: TimePoint | None = None
    degradations: tuple[str, ...] = ()


class SymbolCatalogEntry(Contract):
    key: str
    record: CognitiveRecord


class SymbolCarrier(Contract):
    key: str
    reference: MediaReference
    attachment_index: int


class SymbolSemanticInput(Contract):
    mode: Literal["understand", "express"]
    selection: SymbolSelection
    at: TimePoint
    entities: tuple[SymbolCatalogEntry, ...]
    messages: tuple[SymbolCatalogEntry, ...]
    conventions: tuple[SymbolCatalogEntry, ...]
    relations: tuple[SymbolCatalogEntry, ...]
    carriers: tuple[SymbolCarrier, ...]
    locale: str | None = Field(
        default=None, min_length=1, max_length=512, exclude_if=lambda value: value is None
    )


class SymbolDecisionMatch(Contract):
    key: str
    score: float = Field(ge=0, le=1, allow_inf_nan=False)
    confidence: float | None = Field(default=None, ge=0, le=1, allow_inf_nan=False)
    guidance: str
    source_keys: tuple[str, ...]


class SymbolLocalDraft(Contract):
    content_description: str
    meaning: str
    expression: str = ""
    emotions: tuple[str, ...] = ()
    intentions: tuple[str, ...] = ()
    confidence: float | None = Field(default=None, ge=0, le=1, allow_inf_nan=False)
    guidance: str = ""
    alternatives: tuple[str, ...] = ()
    unknown_aspects: tuple[str, ...] = ()
    source_keys: tuple[str, ...]

    @field_validator(
        "emotions",
        "intentions",
        "alternatives",
        "unknown_aspects",
        "source_keys",
        mode="before",
        json_schema_input_type=str | tuple[str, ...],
    )
    @classmethod
    def text_items(cls, value: object) -> object:
        """A provider may return one complete item or a list; preserve each item verbatim."""
        return (value,) if isinstance(value, str) else value


class SymbolDecision(Contract):
    matches: tuple[SymbolDecisionMatch, ...] = ()
    local_interpretation: SymbolLocalDraft | None = None
    ambiguous: bool = False
