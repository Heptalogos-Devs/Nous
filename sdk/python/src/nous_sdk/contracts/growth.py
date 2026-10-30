"""Reviewable cognitive proposals bind symbolic keys to an actual knowledge snapshot."""

from typing import Annotated, Literal

from pydantic import Field, JsonValue

from .cognition import CognitiveRecord
from .common import Contract
from .growth_bodies import GrowthBody


class GrowthEntry(Contract):
    key: str
    record: CognitiveRecord


class GrowthSnapshot(Contract):
    watermark: int
    entries: tuple[GrowthEntry, ...]
    material_ids: tuple[str, ...]
    capabilities: tuple[str, ...] = ()


class GrowthBinding(Contract):
    path: str = Field(min_length=1)
    key: str = Field(min_length=1)


class GrowthAdoption(Contract):
    action: Literal["adopt"]
    key: str = Field(
        min_length=1, description="Unique proposal local key exposed after this action is adopted"
    )
    target_key: str | None = Field(
        default=None, description="Snapshot key to revise; omission creates a new identity"
    )
    body_input: dict[str, JsonValue]
    bindings: tuple[GrowthBinding, ...] = ()
    evidence_keys: tuple[str, ...] = Field(
        min_length=1, description="Actual supporting snapshot or earlier action keys"
    )
    reason: str = Field(min_length=1)


class GrowthMerge(Contract):
    action: Literal["merge_concepts"]
    key: str = Field(min_length=1)
    target_key: str
    source_keys: tuple[str, ...] = Field(min_length=1)
    reason: str = Field(min_length=1)


class GrowthPartition(Contract):
    key: str = Field(min_length=1)
    label: str = Field(min_length=1)
    member_keys: tuple[str, ...] = Field(min_length=1)


class GrowthSplit(Contract):
    action: Literal["split_concept"]
    key: str = Field(min_length=1, description="Local key for the retired source revision")
    source_key: str
    partitions: tuple[GrowthPartition, ...] = Field(min_length=2)
    reason: str = Field(min_length=1)


class GrowthMemoryChange(Contract):
    action: Literal["memory_change"]
    key: str = Field(min_length=1)
    target_key: str
    transition: Literal[
        "reinforce", "weaken", "protect", "unprotect", "freeze", "forget", "restore"
    ]
    strength: float = Field(default=1, ge=0, allow_inf_nan=False)
    hours: float | None = Field(default=None, ge=0, le=1000000, allow_inf_nan=False)
    reason: str = Field(min_length=1)


GrowthAction = Annotated[
    GrowthAdoption | GrowthMerge | GrowthSplit | GrowthMemoryChange, Field(discriminator="action")
]


class GrowthDraft(Contract):
    summary: str = Field(min_length=1)
    rationale: str = Field(min_length=1)
    actions: tuple[GrowthAction, ...] = Field(default=(), max_length=100)


class GrowthProposalAdoption(Contract):
    action: Literal["adopt"]
    key: str = Field(min_length=1)
    target_key: str | None = None
    body: GrowthBody
    evidence_keys: tuple[str, ...] = Field(min_length=1)
    reason: str = Field(min_length=1)


GrowthProposalAction = Annotated[
    GrowthProposalAdoption | GrowthMerge | GrowthSplit | GrowthMemoryChange,
    Field(discriminator="action"),
]


class GrowthProposal(Contract):
    summary: str = Field(min_length=1)
    rationale: str = Field(min_length=1)
    actions: tuple[GrowthProposalAction, ...] = Field(default=(), max_length=100)
