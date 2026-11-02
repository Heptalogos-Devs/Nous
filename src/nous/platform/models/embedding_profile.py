"""A resolved embedding route snapshot, with no credential values."""

from collections.abc import Mapping
from dataclasses import dataclass
from hashlib import sha256

from nous_sdk.contracts.common import Contract
from nous_sdk.contracts.model_roles import ModelCachePolicy
from pydantic import Field, JsonValue, model_validator

from nous.platform.configuration import EmbeddingRoute
from nous.platform.persistence.authority import canonical_json


class EmbeddingProfile(Contract):
    route: EmbeddingRoute
    fallbacks: tuple[EmbeddingRoute, ...] = ()
    route_ids: tuple[str, ...] = ()
    cache_policy: ModelCachePolicy = "shared"

    @model_validator(mode="after")
    def route_identities(self) -> EmbeddingProfile:
        if self.route_ids and len(self.route_ids) != len(self.fallbacks) + 1:
            raise ValueError("Embedding profile route identities must match its route directory")
        return self

    def resource_key(self) -> str:
        route = self.route
        # Provider identity and vector geometry define the mathematical input space.
        values: tuple[object, ...] = (
            route.base_url,
            route.protocol,
            route.model,
            route.dimensions,
            route.normalization,
            route.extra_body,
        )
        if self.fallbacks:
            values = (
                *values,
                tuple(
                    (
                        item.base_url,
                        item.protocol,
                        item.model,
                        item.dimensions,
                        item.normalization,
                        item.extra_body,
                    )
                    for item in self.fallbacks
                ),
            )
        return sha256(canonical_json(values).encode()).hexdigest()


@dataclass(frozen=True)
class EmbeddingSpaceEvidence:
    space: str
    model: str
    dimensions: int
    route_indexes: tuple[int, ...] = ()
