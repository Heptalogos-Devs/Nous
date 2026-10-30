"""Reuse already paid vectors only when their actual invocation proves the input."""

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError, MediaEmbeddingInput

from nous.platform.persistence import ReadView, WriteTx

from .embedding import retained_embedding
from .embedding_profile import EmbeddingEvidence, EmbeddingProfile

if TYPE_CHECKING:
    from . import ModelService


async def reusable_embedding_in(
    self: ModelService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    call_id: str,
    profile: EmbeddingProfile,
    *,
    text: str | None,
    media: MediaEmbeddingInput | None,
    source_ids: tuple[str, ...],
) -> EmbeddingEvidence | None:
    try:
        paid = await self.ledger.successful_in(view, ctx, call_id)
    except DomainError as error:
        if error.code == "model_call_unavailable":
            return None
        raise
    request = paid.request
    route = next(
        (
            route
            for route in (profile.route, *profile.fallbacks)
            if not any(
                request.get(key) != value
                for key, value in (
                    ("model", route.model),
                    ("base_url", route.base_url),
                    ("dimensions", route.dimensions),
                    ("normalization", route.normalization),
                    ("extra_body", route.extra_body),
                )
            )
        ),
        None,
    )
    if route is None:
        return None
    recorded = request.get("source_ids")
    if not isinstance(recorded, list) or any(not isinstance(item, str) for item in recorded):
        return None
    sources = tuple(item for item in recorded if isinstance(item, str))
    if not set(source_ids).issubset(sources):
        return None
    index: int | None = None
    if text is not None:
        texts = request.get("texts")
        if isinstance(texts, list):
            index = next((i for i, value in enumerate(texts) if value == text), None)
    elif media is not None and request.get("path") == route.media_path:
        if request.get("parts") == [media.model_dump(mode="json")]:
            index = 0
    if index is None:
        return None
    batch = retained_embedding(paid.result, paid.id)
    if index >= len(batch.vectors):
        raise DomainError("invalid_embedding", "Retained embedding input and vector counts differ")
    return EmbeddingEvidence(
        batch=batch.model_copy(
            update={
                "vectors": (batch.vectors[index],),
                "call_ids": (call_id,),
            }
        ),
        source_ids=sources,
    )
