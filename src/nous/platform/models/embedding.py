"""Configured OpenAI-compatible content-parts extension for joint media vectors."""

import asyncio
import base64
import hashlib
import json
from collections.abc import Awaitable, Callable

from nous_wave.contracts import CallContext, DomainError, EmbeddingBatch, MediaEmbeddingInput
from openai import AsyncOpenAI
from pydantic import BaseModel, Field, JsonValue

from nous.platform.configuration import EmbeddingRoute
from nous.platform.errors import Unavailable
from nous.platform.network.service import NetworkService

from .generation import MediaInput
from .ledger import CallLedger

MediaResolver = Callable[[CallContext, MediaEmbeddingInput], Awaitable[MediaInput]]


class EmbeddingData(BaseModel):
    embedding: tuple[float, ...]
    index: int = 0


class EmbeddingResponse(BaseModel):
    model: str
    data: EmbeddingData | tuple[EmbeddingData, ...]
    usage: dict[str, JsonValue] = Field(default_factory=dict)


def vector_space(route: EmbeddingRoute, effective_model: str) -> str:
    options = (
        json.dumps(route.extra_body, sort_keys=True, separators=(",", ":"))
        if route.extra_body
        else ""
    )
    return hashlib.sha256(
        f"{route.base_url}:{effective_model}:{route.dimensions}:{route.normalization}{options}".encode()
    ).hexdigest()


def retained_embedding(value: object, call_id: str) -> EmbeddingBatch:
    result = EmbeddingBatch.model_validate(value)
    if result.call_ids != (call_id,) * len(result.vectors):
        raise DomainError(
            "embedding_producer_conflict",
            "Retained embedding producers differ from the actual paid call",
            call_id=call_id,
        )
    return result


async def media_parts(
    ctx: CallContext, group: tuple[str | MediaEmbeddingInput, ...], resolver: MediaResolver
) -> list[dict[str, JsonValue]]:
    parts: list[dict[str, JsonValue]] = []
    for part in group:
        if isinstance(part, str):
            parts.append({"type": "text", "text": part})
            continue
        resolved = await resolver(ctx, part)
        if resolved.media_type.startswith("text/") or resolved.media_type == "application/json":
            parts.append({"type": "text", "text": resolved.data.decode("utf-8")})
            continue
        kind = (
            "image_url"
            if resolved.media_type.startswith("image/")
            else "video_url"
            if resolved.media_type.startswith("video/")
            else None
        )
        if kind is None:
            raise Unavailable(f"Content-parts embeddings do not accept {resolved.media_type}")
        url = f"data:{resolved.media_type};base64," + base64.b64encode(resolved.data).decode()
        parts.append({"type": kind, kind: {"url": url}})
    return parts


async def embed_media(
    ctx: CallContext,
    inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
    route: EmbeddingRoute,
    key: str,
    resolver: MediaResolver,
    ledger: CallLedger,
    network: NetworkService,
    source_ids: tuple[str, ...],
    *,
    invocation_id: str | None = None,
) -> EmbeddingBatch:
    vectors: list[tuple[float, ...]] = []
    call_ids: list[str | None] = []
    effective_model: str | None = None
    for index, group in enumerate(inputs):
        if not group:
            raise ValueError("A joint embedding input must contain at least one part")
        snapshot: dict[str, object] = {
            "model": route.model,
            "base_url": route.base_url,
            "path": route.media_path,
            "source_ids": list(source_ids),
            "parts": [
                part if isinstance(part, str) else part.model_dump(mode="json") for part in group
            ],
            "dimensions": route.dimensions,
            "normalization": route.normalization,
            "extra_body": route.extra_body,
        }
        identity = f"{invocation_id}:{index}" if invocation_id is not None else None
        call_id, cached = await ledger.reserve(
            ctx, "embedding", snapshot, route.max_calls_per_day, invocation_id=identity
        )
        if cached is not None:
            result = retained_embedding(cached, call_id)
            if effective_model is not None and effective_model != result.model:
                raise ValueError("Joint inputs resolve to different embedding model versions")
            effective_model = result.model
            vectors.extend(result.vectors)
            call_ids.extend(result.call_ids)
            continue
        raw: dict[str, JsonValue] | None = None
        try:
            parts = await media_parts(ctx, group, resolver)
            async with network.client(ctx, timeout_seconds=route.timeout_seconds) as client:
                sdk = AsyncOpenAI(
                    base_url=route.base_url, api_key=key, http_client=client, max_retries=0
                )
                raw = await sdk.post(
                    route.media_path,
                    cast_to=dict[str, JsonValue],
                    body={
                        **route.extra_body,
                        "model": route.model,
                        "input": parts,
                        "dimensions": route.dimensions,
                        "encoding_format": "float",
                    },
                )
            response = EmbeddingResponse.model_validate(raw)
            records = (
                (response.data,) if isinstance(response.data, EmbeddingData) else response.data
            )
            if len(records) != 1 or len(records[0].embedding) != route.dimensions:
                raise ValueError("One joint input must produce one vector in its declared space")
            if effective_model is not None and effective_model != response.model:
                raise ValueError("Provider changed model versions within an embedding batch")
            effective_model = response.model
            vector = records[0].embedding
            if route.normalization == "l2":
                import math

                norm = math.sqrt(sum(value * value for value in vector))
                vector = tuple(value / norm for value in vector) if norm else vector
            result = EmbeddingBatch(
                model=response.model,
                space=vector_space(route, response.model),
                dimensions=route.dimensions,
                vectors=(vector,),
                call_ids=(call_id,),
            )
            await ledger.finish(ctx, call_id, result.model_dump(mode="json"), dict(response.usage))
            vectors.append(vector)
            call_ids.extend(result.call_ids)
        except asyncio.CancelledError:
            await asyncio.shield(
                ledger.finish(ctx, call_id, None, {"response": raw}, "CancelledUnknownOutcome")
            )
            raise
        except Exception as error:
            code = str(getattr(error, "status_code", type(error).__name__))
            await ledger.finish(ctx, call_id, None, {"response": raw}, code)
            raise Unavailable(
                f"Media embedding failed ({code})",
                context={
                    "call_id": call_id,
                    "invocation_id": identity,
                    "role": "embedding",
                    "provider_code": code,
                },
            ) from error
    if effective_model is None:
        raise ValueError("Embedding batch cannot be empty")
    return EmbeddingBatch(
        model=effective_model,
        space=vector_space(route, effective_model),
        dimensions=route.dimensions,
        vectors=tuple(vectors),
        call_ids=tuple(call_ids),
    )
