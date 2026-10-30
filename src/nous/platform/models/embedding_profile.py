"""A resolved embedding route snapshot, with no credential values."""

from dataclasses import dataclass
from hashlib import sha256

from nous_sdk.contracts import EmbeddingBatch
from nous_sdk.contracts.common import Contract
from nous_sdk.contracts.model_routing import ModelRoutingPolicy
from pydantic import Field, model_validator

from nous.platform.configuration import EmbeddingRoute
from nous.platform.persistence.authority import canonical_json


class EmbeddingProfile(Contract):
    route: EmbeddingRoute
    fallbacks: tuple[EmbeddingRoute, ...] = ()
    route_ids: tuple[str, ...] = ()
    fallback_policy: ModelRoutingPolicy = Field(default_factory=ModelRoutingPolicy)

    @model_validator(mode="after")
    def route_identities(self) -> EmbeddingProfile:
        if self.route_ids and len(self.route_ids) != len(self.fallbacks) + 1:
            raise ValueError("Embedding profile route identities must match its route directory")
        return self

    def resource_key(self) -> str:
        route = self.route
        # Actual response model and space remain part of each paid content fact.
        # Credentials, timeouts and budgets do not define mathematical input space.
        values: tuple[object, ...] = (
            route.base_url,
            route.model,
            route.dimensions,
            route.normalization,
            route.extra_body,
            route.media_path,
        )
        if self.fallbacks:
            values = (
                *values,
                tuple(
                    (
                        item.base_url,
                        item.model,
                        item.dimensions,
                        item.normalization,
                        item.extra_body,
                        item.media_path,
                    )
                    for item in self.fallbacks
                ),
            )
        return sha256(canonical_json(values).encode()).hexdigest()


@dataclass(frozen=True)
class EmbeddingEvidence:
    batch: EmbeddingBatch
    source_ids: tuple[str, ...]
