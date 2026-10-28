"""The model boundary used by formation; providers live in platform.models."""

from typing import Protocol

from nous_wave.contracts import CallContext, EmbeddingBatch, FormationProposal, MediaEmbeddingInput


class ModelServices(Protocol):
    async def form(
        self, ctx: CallContext, text: str, source: dict[str, object]
    ) -> FormationProposal: ...
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
