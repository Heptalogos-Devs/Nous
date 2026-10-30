"""Canonical cognitive identities and declared evidence shared by every body."""

from __future__ import annotations

from typing import Literal

from pydantic import Field, model_validator

from .common import Contract

CognitiveKind = Literal[
    "memory",
    "memory_semantics",
    "concept",
    "entity",
    "episode",
    "journal",
    "schema",
    "association",
    "self_facet",
    "narrative_thread",
    "domain_knowledge",
    "social_relation",
    "language_convention",
    "desire",
    "concern",
    "fact_claim",
]


class CognitiveIdentity(Contract):
    kind: CognitiveKind
    id: str


class CognitiveRef(Contract):
    kind: CognitiveKind
    id: str
    revision: int = Field(ge=1)

    def exact(self) -> str:
        return f"{self.kind}:{self.id}@{self.revision}"


class EpistemicStance(Contract):
    classification: Literal[
        "observed", "reported", "derived", "inferred", "narrative", "simulated"
    ] = "reported"
    attribution: str | None = None
    confidence: float | None = Field(default=None, ge=0, le=1)


class EvidenceLink(Contract):
    observation_id: str | None = None
    reference: CognitiveRef | None = None
    region_id: str | None = None
    polarity: Literal["support", "counterexample", "boundary"] = "support"
    role: str = "grounding"

    @model_validator(mode="after")
    def source_identity(self) -> EvidenceLink:
        if (self.observation_id is None) == (self.reference is None):
            raise ValueError("Evidence requires exactly one observation or cognitive revision")
        if self.region_id is not None and self.observation_id is None:
            raise ValueError("A source region requires its actual observation")
        return self
