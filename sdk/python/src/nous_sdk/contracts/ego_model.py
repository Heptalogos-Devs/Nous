"""Explicit configuration documents remain independently sourced Ego evidence."""

from typing import Annotated, Literal

from pydantic import Field

from .common import Contract, Operation
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
from .media import MediaReference


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


class SaveSeed(Contract):
    documents: tuple[ImportDocument | MediaReference, ...] = Field(min_length=1, max_length=100)
    instruction: str = Field(default="", max_length=16000)
    seed_id: str | None = None
    expected_version: int = Field(default=0, ge=0)


class SeedVersion(Contract):
    seed_id: str
    version: int = Field(ge=1)
    namespace_id: str
    instruction: str
    sources: tuple[MediaReference, ...]
    created_by: str
    created_at: str
    commit_seq: int
