"""Explicit configuration documents remain independently sourced Ego evidence."""

from typing import Annotated, Literal

from pydantic import Field

from .cognition import CognitiveRef
from .common import Contract, Operation
from .growth import GrowthSource
from .growth_bodies import (
    GrowthAssociation,
    GrowthConcern,
    GrowthDesire,
    GrowthDomainKnowledge,
    GrowthEgoFacet,
    GrowthEntity,
    GrowthLanguageConvention,
    GrowthMemory,
    GrowthNarrativeThread,
    GrowthSocialRelation,
)
from .imports import ImportDocument


class InterpretSeed(Operation):
    documents: tuple[ImportDocument, ...] = Field(min_length=1, max_length=100)
    instruction: str = Field(default="", max_length=16000)
    previous_seed_id: str | None = None


SeedBody = Annotated[
    GrowthMemory
    | GrowthEntity
    | GrowthAssociation
    | GrowthEgoFacet
    | GrowthNarrativeThread
    | GrowthDomainKnowledge
    | GrowthSocialRelation
    | GrowthLanguageConvention
    | GrowthDesire
    | GrowthConcern,
    Field(discriminator="kind"),
]


class SeedAdoption(Contract):
    action: Literal["adopt"] = "adopt"
    key: str = Field(min_length=1)
    target_key: str | None = None
    body: SeedBody
    evidence_keys: tuple[str, ...] = Field(min_length=1)
    reason: str = Field(min_length=1)


class SeedProposal(Contract):
    summary: str = Field(min_length=1)
    rationale: str = Field(min_length=1)
    actions: tuple[SeedAdoption, ...] = Field(default=(), max_length=100)
    questions: tuple[str, ...] = ()


class SeedVersion(Contract):
    id: str
    previous_seed_id: str | None = None
    instruction: str
    created_at: str
    sources: tuple[GrowthSource, ...]
    adopted: tuple[CognitiveRef, ...] = ()
    review: Literal["adopt", "reject"] | None = None
