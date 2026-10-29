"""Bounded provider-neutral text embedding batches share actual paid results."""

import asyncio

from nous_wave.contracts import CallContext, EmbeddingBatch
from openai import AsyncOpenAI

from nous.platform.configuration import EmbeddingRoute
from nous.platform.errors import Unavailable
from nous.platform.network.service import NetworkService

from .embedding import retained_embedding, vector_space
from .ledger import CallLedger


async def embed_one(
    ctx: CallContext,
    texts: tuple[str, ...],
    route: EmbeddingRoute,
    key: str,
    ledger: CallLedger,
    network: NetworkService,
    source_ids: tuple[str, ...],
) -> EmbeddingBatch:
    request: dict[str, object] = {
        "model": route.model,
        "base_url": route.base_url,
        "texts": list(texts),
        "source_ids": list(source_ids),
        "dimensions": route.dimensions,
        "normalization": route.normalization,
        "extra_body": route.extra_body,
    }
    call_id, cached = await ledger.reserve(ctx, "embedding", request, route.max_calls_per_day)
    if cached is not None:
        return retained_embedding(cached, call_id)
    try:
        async with network.client(ctx, timeout_seconds=route.timeout_seconds) as client:
            sdk = AsyncOpenAI(
                base_url=route.base_url, api_key=key, http_client=client, max_retries=0
            )
            response = await sdk.embeddings.create(
                model=route.model,
                input=list(texts),
                dimensions=route.dimensions,
                extra_body=route.extra_body,
            )
        vectors = [tuple(item.embedding) for item in sorted(response.data, key=lambda v: v.index)]
        if len(vectors) != len(texts) or any(len(v) != route.dimensions for v in vectors):
            raise ValueError("Provider returned inconsistent embedding dimensions")
        if route.normalization == "l2":
            import numpy as np

            array = np.asarray(vectors, dtype=np.float64)
            norms = np.linalg.norm(array, axis=1, keepdims=True)
            vectors = [tuple(row) for row in array / np.where(norms == 0, 1, norms)]
        space = vector_space(route, response.model)
        result = EmbeddingBatch(
            model=response.model,
            space=space,
            dimensions=route.dimensions,
            vectors=tuple(vectors),
            call_ids=(call_id,) * len(vectors),
        )
        await ledger.finish(ctx, call_id, result.model_dump(mode="json"), {})
        return result
    except asyncio.CancelledError:
        await asyncio.shield(ledger.finish(ctx, call_id, None, {}, "CancelledUnknownOutcome"))
        raise
    except Exception as error:
        code = str(getattr(error, "status_code", type(error).__name__))
        await ledger.finish(ctx, call_id, None, {}, code)
        raise Unavailable(
            f"Embedding provider failed ({code})",
            context={
                "call_id": call_id,
                "invocation_id": None,
                "role": "embedding",
                "provider_code": code,
            },
        ) from error


async def embed_text(
    ctx: CallContext,
    texts: tuple[str, ...],
    route: EmbeddingRoute,
    key: str,
    ledger: CallLedger,
    network: NetworkService,
    source_ids: tuple[str, ...],
) -> EmbeddingBatch:
    if not texts or any(not text.strip() for text in texts):
        raise ValueError("Text embedding inputs must contain nonempty text")
    batches: list[EmbeddingBatch] = []
    for offset in range(0, len(texts), route.max_batch_items):
        batch = await embed_one(
            ctx,
            texts[offset : offset + route.max_batch_items],
            route,
            key,
            ledger,
            network,
            source_ids,
        )
        if batches and (batch.space, batch.model, batch.dimensions) != (
            batches[0].space,
            batches[0].model,
            batches[0].dimensions,
        ):
            raise ValueError("Provider changed embedding spaces within one input batch")
        batches.append(batch)
    return EmbeddingBatch(
        model=batches[0].model,
        space=batches[0].space,
        dimensions=batches[0].dimensions,
        vectors=tuple(vector for batch in batches for vector in batch.vectors),
        call_ids=tuple(call_id for batch in batches for call_id in batch.call_ids),
    )
