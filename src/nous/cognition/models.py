"""The model boundary used by formation; providers live in platform.models."""

from __future__ import annotations

from typing import TYPE_CHECKING, Protocol

from pydantic import BaseModel

if TYPE_CHECKING:
    from nous.platform.models.embedding_profile import EmbeddingEvidence, EmbeddingProfile
    from nous.platform.models.generation import Generated, MediaInput
    from nous.platform.persistence import ReadView, WriteTx

from nous_wave.contracts import CallContext, EmbeddingBatch, MediaEmbeddingInput
from nous_wave.contracts.model_roles import ModelCachePolicy


class ModelServices(Protocol):
    async def generate[T: BaseModel | str](
        self,
        ctx: CallContext,
        role_id: str,
        variables: dict[str, object],
        output_type: type[T],
        *,
        source_ids: tuple[str, ...] = (),
        invocation_id: str | None = None,
        cache_policy: ModelCachePolicy = "shared",
        media: tuple[MediaInput, ...] = (),
    ) -> Generated[T]: ...
    async def embed(
        self,
        ctx: CallContext,
        texts: tuple[str, ...],
        *,
        source_ids: tuple[str, ...] = (),
        invocation_id: str | None = None,
        profile: EmbeddingProfile | None = None,
    ) -> EmbeddingBatch: ...
    async def prepare_embedding(self, ctx: CallContext) -> EmbeddingProfile: ...
    async def successful_embedding_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        call_id: str,
    ) -> EmbeddingBatch: ...
    async def reusable_embedding_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        call_id: str,
        profile: EmbeddingProfile,
        *,
        text: str | None,
        media: MediaEmbeddingInput | None,
        source_ids: tuple[str, ...],
    ) -> EmbeddingEvidence | None: ...
    async def embed_media(
        self,
        ctx: CallContext,
        inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
        *,
        source_ids: tuple[str, ...] = (),
        invocation_id: str | None = None,
        profile: EmbeddingProfile | None = None,
    ) -> EmbeddingBatch: ...
