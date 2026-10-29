"""A resolved embedding route snapshot, with no credential values."""

from dataclasses import dataclass
from hashlib import sha256

from nous_wave.contracts import EmbeddingBatch
from nous_wave.contracts.common import Contract

from nous.platform.configuration import EmbeddingRoute
from nous.platform.persistence.authority import canonical_json


class EmbeddingProfile(Contract):
    route: EmbeddingRoute

    def resource_key(self) -> str:
        route = self.route
        # Actual response model and space remain part of each paid content fact.
        # Credentials, timeouts and budgets do not define mathematical input space.
        return sha256(
            canonical_json(
                (
                    route.base_url,
                    route.model,
                    route.dimensions,
                    route.normalization,
                    route.extra_body,
                    route.media_path,
                )
            ).encode()
        ).hexdigest()


@dataclass(frozen=True)
class EmbeddingEvidence:
    batch: EmbeddingBatch
    source_ids: tuple[str, ...]
