"""Request-owned graph intent, transient query concepts and grounded routes."""

from __future__ import annotations

from typing import Literal

from pydantic import Field, FiniteFloat

from .cognition import CognitiveRef
from .common import Contract
from .media import MediaReference


class QueryConcept(Contract):
    text: str = Field(min_length=1, max_length=16384)
    is_core: bool = False


class VcpIntent(Contract):
    core: tuple[CognitiveRef, ...] = Field(default=(), max_length=256)
    concepts: tuple[QueryConcept, ...] = Field(default=(), max_length=256)
    thinking: bool | None = None


class QueryConceptResult(Contract):
    concept: QueryConcept
    call_id: str
    space: str


class ThinkingRoute(Contract):
    source: CognitiveRef
    target: CognitiveRef
    previous: CognitiveRef | None
    hop: int = Field(ge=0)
    flow: FiniteFloat = Field(ge=0)
    wormhole: bool
    immediate_return: bool


class ThinkingSelection(Contract):
    reference: CognitiveRef | MediaReference
    score: FiniteFloat
    parents: tuple[CognitiveRef | MediaReference, ...]
    relation: Literal["root", "continuation", "analogy", "supplement"]
    route: ThinkingRoute | None


class ThinkingStage(Contract):
    name: str
    limit: int = Field(ge=0, le=32)
    selections: tuple[ThinkingSelection, ...]


class QueryThinking(Contract):
    status: Literal["ready", "unavailable"]
    reason: str | None = None
    generation: str | None = None
    artifact_signature: str | None = None
    stages: tuple[ThinkingStage, ...] = ()


class VcpResult(Contract):
    core: tuple[CognitiveRef, ...] = ()
    concepts: tuple[QueryConceptResult, ...] = ()
    thinking: QueryThinking | None = None
    branches: tuple[VcpResult, ...] = ()
