"""Vector-space storage, configured coverage and durable projection maintenance."""

from typing import Literal

from pydantic import Field, model_validator

from .cognition import CognitiveKind, CognitiveRef
from .common import Contract, Operation, TimePoint
from .common import ModelProvenance
from .media import MediaReference
from .vector_projection import ProjectionReceipt


class VectorSpace(Contract):
    space_id: str
    model: str
    dimensions: int | None
    configured: bool = False
    route_ids: tuple[str, ...] = ()
    vector_count: int = 0
    invalid_vectors: int = 0
    payload_bytes: int = 0
    missing_references: int | None = None
    can_delete: bool = False
    delete_scope: Literal["namespace"] = "namespace"
    deleted_at_commit: int | None = None


class VectorSpaces(Contract):
    namespace_id: str
    
    authority_commit_seq: int
    spaces: tuple[VectorSpace, ...]
    eligible_references: int
    configured_space_complete: bool | None
    configured: bool
    include_media: bool = False


class DeleteVectorSpace(Operation):
    space_id: str
    reason: str = "Delete derived vector space"


class VectorSpaceDeleted(Contract):
    space_id: str
    vectors_deleted: int
    contents_deleted: int
    commit_seq: int


class RebuildVectors(Operation):
    mode: Literal["missing", "all"] = "missing"
    kinds: tuple[CognitiveKind, ...] = ()
    include_media: bool = False
    limit: int = Field(default=10000, ge=0, le=10000)
    dry_run: bool = False
    batch_size: int | None = Field(default=None, ge=1, le=10000, exclude_if=lambda value: value is None)


class VectorRebuild(Contract):
    mode: Literal["missing", "all"]
    references: tuple[CognitiveRef | MediaReference, ...]
    total_eligible: int
    remaining: int
    projection: ProjectionReceipt | None = None
    commit_seq: int


class RecoverVectors(Operation):
    max_retry: int = Field(default=3, ge=0, le=10)
    limit: int = Field(default=100, ge=1, le=10000)
    kinds: tuple[CognitiveKind, ...] = ()


class VectorRecovery(Contract):
    resumed: tuple[ProjectionReceipt, ...]
    backfill: VectorRebuild


class RetryVectorProjection(Operation):
    request_id: str


class VectorProjectionProgress(Contract):
    request_id: str
    job_id: str
    status: str
    targets: int
    prepared_contents: int
    adopted_references: int
    spaces: tuple[str, ...] = ()
    error: str | None = None


class CheckVectorSpaces(Operation):
    probe: bool = False
    sample_text: str | None = Field(default=None, min_length=1, max_length=16000)
    sample_reference: CognitiveRef | None = None

    @model_validator(mode="after")
    def probe_input(self) -> "CheckVectorSpaces":
        supplied = int(self.sample_text is not None) + int(self.sample_reference is not None)
        if self.probe and supplied != 1 or not self.probe and supplied:
            raise ValueError("provider probing requires exactly one actual text or cognitive reference")
        if self.sample_text is not None and not self.sample_text.strip():
            raise ValueError("provider probe text must not be blank")
        return self


class VectorSpaceCheck(Contract):
    checked_at: TimePoint
    authority_commit_seq: int
    checked_vectors: int
    invalid_references: tuple[str, ...]
    configured: bool
    credential_ready: bool
    provider_probed: bool
    provider_ready: bool | None = None
    probe_space: str | None = None
    producer: ModelProvenance | None = None


class PersistenceState(Contract):
    auto_save: Literal[True] = True
    mode: Literal["transaction_commit"] = "transaction_commit"
    committed_sequence: int
