"""Native namespace admission and formation policy, independent of adapters."""
from pydantic import Field, model_validator

from .common import Contract


class NamespaceQuota(Contract):
    max_concurrent_requests: int | None = Field(default=None, ge=1)
    max_storage_bytes: int | None = Field(default=None, ge=1)

class FormationPolicy(Contract):
    enabled: bool = True
    default_enabled: bool = False
    coalesce_seconds: float = Field(default=30, ge=0, le=3600)
    text_input_tokens: int = Field(default=4096, ge=256)
    text_output_tokens: int = Field(default=2048, ge=64)
    profile: str = 'native'
    entity_types: tuple[str, ...] = ()
    predicates: tuple[str, ...] = ()
    max_entities: int | None = Field(default=None, ge=1, le=256)
    max_relations: int | None = Field(default=None, ge=1, le=256)
    max_chunk_chars: int = Field(default=100000, ge=128, le=100000)
    chunk_overlap_chars: int = Field(default=200, ge=0)

    @model_validator(mode='after')
    def valid_policy(self) -> 'FormationPolicy':
        if self.profile != 'native':
            raise ValueError('Unknown native formation profile')
        if self.default_enabled and (not self.enabled):
            raise ValueError('Default formation requires formation enabled')
        if self.chunk_overlap_chars >= self.max_chunk_chars:
            raise ValueError('Formation overlap must be smaller than chunk size')
        return self
