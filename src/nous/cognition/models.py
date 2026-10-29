"""The model boundary used by formation; providers live in platform.models."""

from __future__ import annotations

from typing import TYPE_CHECKING, Protocol

from pydantic import BaseModel

if TYPE_CHECKING:
    from nous.platform.models.generation import Generated, MediaInput

from nous_wave.contracts import CallContext, EmbeddingBatch, MediaEmbeddingInput


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
        media: tuple[MediaInput, ...] = (),
    ) -> Generated[T]: ...
    async def embed(
        self, ctx: CallContext, texts: tuple[str, ...], *, source_ids: tuple[str, ...] = ()
    ) -> EmbeddingBatch: ...
    async def embed_media(
        self,
        ctx: CallContext,
        inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
        *,
        source_ids: tuple[str, ...] = (),
    ) -> EmbeddingBatch: ...
