"""A claim's identity, classification and explicit support remain canonical cognition."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import Field, JsonValue, model_validator

from .cognitive_identity import CognitiveRef, EvidenceLink
from .common import Contract, TimePoint

FactStatus = Literal["active", "conflicted", "superseded", "retracted"]
FactAuthority = Literal["manual", "direct_user", "imported", "summary_derived"]
FactStability = Literal["stable", "temporal", "uncertain"]
FactSection = Literal[
    "identity_settings",
    "relationship_settings",
    "stable_facts",
    "interaction_preferences",
    "recent_interactions",
    "uncertain_notes",
]


class PersonFactScope(Contract):
    kind: Literal["person"] = "person"
    person: CognitiveRef

    @model_validator(mode="after")
    def entity_reference(self) -> PersonFactScope:
        if self.person.kind != "entity":
            raise ValueError("A person claim requires an exact Person Entity reference")
        return self

    @property
    def identity(self) -> str:
        return self.person.id


class ChatFactScope(Contract):
    kind: Literal["chat"] = "chat"
    conversation_ref: str = Field(min_length=1)

    @property
    def identity(self) -> str:
        return self.conversation_ref


FactScope = Annotated[PersonFactScope | ChatFactScope, Field(discriminator="kind")]


class FactEvidence(Contract):
    link: EvidenceLink
    weight: float | None = Field(default=None, ge=0, le=1)
    metadata: dict[str, JsonValue] = Field(default_factory=dict)

    @model_validator(mode="after")
    def claim_stance(self) -> FactEvidence:
        if self.link.polarity == "boundary":
            raise ValueError("Claim evidence must explicitly support or refute")
        return self


class FactClaimValue(Contract):
    fact_key: str = Field(min_length=1)
    value_text: str = Field(min_length=1)
    polarity: Literal["positive", "negative"] = "positive"
    cardinality: Literal["single", "set"] = "set"
    stability: FactStability = "stable"
    profile_section: FactSection = "stable_facts"
    authority: FactAuthority = "manual"
    confidence: float | None = Field(default=None, ge=0, le=1)


class FactClaimSpec(FactClaimValue):
    scope: FactScope


class FactClaimBody(FactClaimSpec):
    kind: Literal["fact_claim"] = "fact_claim"
    status: FactStatus = "active"
    evidence: tuple[FactEvidence, ...] = ()
    supersedes: tuple[CognitiveRef, ...] = ()
    first_observed: TimePoint | None = None
    last_confirmed: TimePoint | None = None
