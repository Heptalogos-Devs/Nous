"""Explicit applicability of a personality facet to actual social knowledge."""

from typing import Literal

from pydantic import Field

from .cognitive_identity import CognitiveRef
from .common import Contract

SocialDimension = Literal[
    "familiarity",
    "affinity",
    "intimacy",
    "trust",
    "cooperation",
    "guidance",
    "membership",
    "conflict",
    "other",
]
FacetCategory = Literal["identity", "value", "preference", "tendency", "interest", "other"]


class RelationshipRequirement(Contract):
    from_entity: CognitiveRef
    dimension: SocialDimension
    minimum_degree: float = Field(default=0, ge=-1, le=1, allow_inf_nan=False)
    minimum_confidence: float | None = Field(default=None, ge=0, le=1)
    trust_domain: str | None = None
