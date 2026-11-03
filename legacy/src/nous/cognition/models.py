"""The model boundary used by formation; providers live in platform.models."""

from __future__ import annotations

from typing import TYPE_CHECKING, Protocol

from nous_sdk.contracts.prompt_compilation import PromptMessage
from typing_extensions import TypeForm

if TYPE_CHECKING:
    from nous.platform.models.embedding_profile import (
        EmbeddingProfile,
        EmbeddingSpaceEvidence,
    )
    from nous.platform.models.generation import MediaInput, ModelResult
    from nous.platform.persistence import ReadView, WriteTx
from nous_sdk.contracts import EmbeddingBatch, MediaEmbeddingInput
from nous.platform.call_context import CallContext
from nous_sdk.contracts.model_roles import ModelCachePolicy


class ModelServices(Protocol):
    async def configured_embedding_spaces_in(
        self, view: ReadView | WriteTx, ctx: CallContext, profile: EmbeddingProfile
    ) -> tuple[EmbeddingSpaceEvidence, ...]: ...

    async def generate[T](
        self,
        ctx: CallContext,
        role_id: str,
        variables: dict[str, object],
        output_type: TypeForm[T],
        *,
        source_ids: tuple[str, ...] = (),
        cache_policy: ModelCachePolicy = "none",
        allow_fallback: bool = False,
        media: tuple[MediaInput, ...] = (),
        context_messages: tuple[PromptMessage, ...] = (),
        max_output_tokens: int | None = None,
    ) -> ModelResult[T]: ...

    async def embed(
        self,
        ctx: CallContext,
        texts: tuple[str, ...],
        *,
        source_ids: tuple[str, ...] = (),
        profile: EmbeddingProfile | None = None,
        cache_policy: ModelCachePolicy | None = None,
        allow_fallback: bool = False,
    ) -> ModelResult[EmbeddingBatch]: ...

    async def prepare_embedding(self, ctx: CallContext) -> EmbeddingProfile: ...

    async def embed_media(
        self,
        ctx: CallContext,
        inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
        *,
        source_ids: tuple[str, ...] = (),
        profile: EmbeddingProfile | None = None,
        cache_policy: ModelCachePolicy | None = None,
        allow_fallback: bool = False,
    ) -> ModelResult[EmbeddingBatch]: ...
