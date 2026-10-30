"""Explicit configuration documents remain independently sourced Ego evidence."""

from pydantic import Field

from .cognition import CognitiveRef
from .common import Contract, Operation
from .imports import ImportDocument


class SeedFacet(Contract):
    facet: str = Field(min_length=1)
    description: str = Field(min_length=1)
    contexts: tuple[str, ...] = ()
    exceptions: tuple[str, ...] = ()
    target: CognitiveRef | None = None


class SeedDocument(Contract):
    document: ImportDocument
    facets: tuple[SeedFacet, ...] = Field(min_length=1)
    enter_memory: bool = True


class AdoptSeed(Operation):
    documents: tuple[SeedDocument, ...] = Field(min_length=1)


class SeedResult(Contract):
    observation_ids: tuple[str, ...]
    memories: tuple[CognitiveRef, ...]
    facets: tuple[CognitiveRef, ...]
    commit_seq: int
