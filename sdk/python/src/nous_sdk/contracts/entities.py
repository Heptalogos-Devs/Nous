"""Source-backed entity identity merge and explicitly assigned split operations."""

from pydantic import Field

from .cognition import CognitiveRef, EntityBody, ReferencePartition
from .common import Contract, Operation


class MergeEntities(Operation):
    target: CognitiveRef
    sources: tuple[CognitiveRef, ...] = Field(min_length=1)
    reason: str = Field(min_length=1)


class EntityPartition(Contract):
    body: EntityBody
    claims: tuple[CognitiveRef, ...] = ()


class SplitEntity(Operation):
    source: CognitiveRef
    partitions: tuple[EntityPartition, ...] = Field(min_length=2)
    consumers: tuple[ReferencePartition, ...] = ()
    reason: str = Field(min_length=1)
