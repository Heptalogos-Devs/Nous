"""Scope-aware observed inventories; configured-model coverage can remain unknown."""

from .common import Contract


class VectorSpaceInventory(Contract):
    space: str
    model: str
    dimensions: int
    indexed_references: int
    missing_references: int


class RetrievalStatus(Contract):
    authority_commit_seq: int
    visible_targets: int
    native_generations: int
    spaces: tuple[VectorSpaceInventory, ...]
    configured_space_complete: bool | None = None


class ImportVectorStatus(Contract):
    authority_commit_seq: int
    visible_outputs: int
    disabled_outputs: int
    required_outputs: int
    metadata_allowed_outputs: int
    outputs_with_complete_adopted_space: int
    outputs_without_vectors: int
    outputs_with_partial_vectors: int
    vector_failed_attempts: int
    running_embedding_attempts: int
    configured_space_complete: bool | None = None
