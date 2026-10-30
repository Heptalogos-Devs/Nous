"""Frozen inputs and producer-bearing content, without a second job state."""

from typing import Literal

from nous_sdk.contracts import CallContext, CognitiveRef, MediaEmbeddingInput, MediaReference
from nous_sdk.contracts.common import Contract

from nous.platform.models.embedding_profile import EmbeddingProfile

Target = CognitiveRef | MediaReference
VectorPolicy = Literal["disabled", "metadata_allowed", "required"]


class Input(Contract):
    key: str
    text: str | None = None
    media: MediaEmbeddingInput | None = None
    source_ids: tuple[str, ...]


class TargetInput(Contract):
    target: Target
    input: Input


class Plan(Contract):
    targets: tuple[TargetInput, ...]
    owned: tuple[Input, ...]
    dependencies: tuple[str, ...]
    omitted: tuple[str, ...] = ()


class Request(Contract):
    id: str
    context: CallContext
    targets: tuple[Target, ...]
    profile: EmbeddingProfile
    head_only: bool
    job_id: str
    plan: Plan | None = None
    purged: bool = False


class Content(Contract):
    input: Input
    space: str
    model: str
    dimensions: int
    vector: tuple[float, ...]
    call_id: str
    call_source_ids: tuple[str, ...]
