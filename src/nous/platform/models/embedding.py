"""Text and media embedding requests through configured provider endpoints."""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import math
from collections.abc import Awaitable, Callable

from nous_sdk.contracts import CallContext, DomainError, EmbeddingBatch, MediaEmbeddingInput
from nous_sdk.contracts.model_roles import ModelCachePolicy
from openai import AsyncOpenAI
from pydantic import BaseModel, Field, JsonValue, TypeAdapter
from pydantic_ai.usage import RunUsage

from nous.platform.configuration import EmbeddingRoute
from nous.platform.errors import Unavailable
from nous.platform.network.service import NetworkService

from .generation import Generation, MediaInput, ModelResult

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
        f"{route.protocol}:{route.base_url}:{effective_model}:{route.dimensions}:{route.normalization}{options}".encode()
    ).hexdigest()


def combine_usage(values: tuple[RunUsage, ...]) -> RunUsage:
    fields = (
        "requests",
        "tool_calls",
        "input_tokens",
        "cache_write_tokens",
        "cache_read_tokens",
        "input_audio_tokens",
        "cache_audio_read_tokens",
        "output_tokens",
    )
    totals = {name: sum(getattr(item, name) for item in values) for name in fields}
    details: dict[str, int] = {}
    for item in values:
        for name, value in item.details.items():
            details[name] = details.get(name, 0) + value
    return RunUsage(**totals, details=details)


def media_usage_from(response: EmbeddingResponse) -> RunUsage:
    input_tokens = int(response.usage.get("prompt_tokens", 0) or 0)
    total_tokens = int(response.usage.get("total_tokens", input_tokens) or input_tokens)
    return RunUsage(
        requests=1,
        input_tokens=input_tokens,
        output_tokens=max(0, total_tokens - input_tokens),
    )


async def media_parts(
    ctx: CallContext,
    group: tuple[str | MediaEmbeddingInput, ...],
    resolver: MediaResolver,
) -> tuple[list[dict[str, JsonValue]], list[dict[str, JsonValue]]]:
    parts: list[dict[str, JsonValue]] = []
    inputs: list[dict[str, JsonValue]] = []
    for ordinal, part in enumerate(group):
        if isinstance(part, str):
            encoded = part.encode("utf-8")
            parts.append({"type": "text", "text": part})
            inputs.append(
                {
                    "ordinal": ordinal,
                    "media_type": "text/plain",
                    "byte_len": len(encoded),
                    "sha256": hashlib.sha256(encoded).hexdigest(),
                }
            )
            continue
        resolved = await resolver(ctx, part)
        inputs.append(
            {
                "ordinal": ordinal,
                "media_type": resolved.media_type,
                "byte_len": len(resolved.data),
                "sha256": hashlib.sha256(resolved.data).hexdigest(),
                "source_reference": resolved.source_reference,
            }
        )
        if resolved.media_type.startswith("text/") or resolved.media_type == "application/json":
            parts.append({"type": "text", "text": resolved.data.decode("utf-8")})
            continue
        encoded = base64.b64encode(resolved.data).decode("ascii")
        if resolved.media_type.startswith("image/") or resolved.media_type.startswith("video/"):
            kind = "image_url" if resolved.media_type.startswith("image/") else "video_url"
            url = f"data:{resolved.media_type};base64,{encoded}"
            parts.append({"type": kind, kind: {"url": url}})
        elif resolved.media_type in {"audio/wav", "audio/x-wav", "audio/mpeg", "audio/mp3"}:
            audio_format = "mp3" if resolved.media_type in {"audio/mpeg", "audio/mp3"} else "wav"
            parts.append({"type": "input_audio", "input_audio": {"data": encoded, "format": audio_format}})
        elif resolved.media_type == "application/pdf":
            url = f"data:{resolved.media_type};base64,{encoded}"
            parts.append({"type": "file", "file": {"filename": "source.pdf", "file_data": url}})
        else:
            raise Unavailable(f"Configured media embeddings do not accept {resolved.media_type}")
    return parts, inputs


async def embed_media(
    ctx: CallContext,
    inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
    route: EmbeddingRoute,
    key: str,
    resolver: MediaResolver,
    network: NetworkService,
    generation: Generation,
    source_ids: tuple[str, ...],
    *,
    cache_policy: ModelCachePolicy = "none",
) -> ModelResult[EmbeddingBatch]:
    if not inputs or any(not group for group in inputs):
        raise ValueError("Media embedding inputs must contain nonempty parts")
    if route.protocol != "volcengine-ark":
        raise DomainError(
            "model_unavailable",
            "The selected embedding API dialect does not accept multimodal input groups",
            reason="embedding_media_dialect_unsupported",
            protocol=route.protocol,
        )
    media_path = "/embeddings/multimodal"
    rendered: list[list[dict[str, JsonValue]]] = []
    input_groups: list[list[dict[str, JsonValue]]] = []
    for group in inputs:
        parts, descriptors = await media_parts(ctx, group, resolver)
        rendered.append(parts)
        input_groups.append(descriptors)
    output_schema = TypeAdapter(EmbeddingBatch).json_schema()
    task = "embedding"
    prompt_digest = generation.digest({"task": task, "protocol": route.protocol, "path": media_path})
    input_digest = generation.digest(input_groups)
    schema_digest = generation.digest(output_schema)
    parameters = {
        "path": media_path,
        "dimensions": route.dimensions,
        "normalization": route.normalization,
        "max_batch_items": route.max_batch_items,
        "extra_body": route.extra_body,
        "timeout_seconds": route.timeout_seconds,
    }
    cache_key = generation.digest(
        {
            "task": task,
            "endpoint": route.base_url,
            "protocol": route.protocol,
            "model": route.model,
            "parameters": parameters,
            "input_digest": input_digest,
            "schema_digest": schema_digest,
        }
    )
    if cache_policy == "shared":
        cached = generation.cache_get(cache_key, EmbeddingBatch)
        if cached is not None:
            generation.record_result(
                ctx,
                task=task,
                model=cached.producer.model_id,
                protocol=route.protocol,
                parameters=TypeAdapter(dict[str, JsonValue]).validate_python(parameters),
                schema=output_schema,
                response_format=None,
                messages=TypeAdapter(JsonValue).validate_python(input_groups),
                media=TypeAdapter(JsonValue).validate_python(input_groups),
                output=cached.output,
                producer=cached.producer,
                usage=cached.usage,
                cache_hit=True,
                elapsed_ms=0,
            )
            return cached
    vectors: list[tuple[float, ...]] = []
    usages: list[RunUsage] = []
    effective_model: str | None = None
    started = asyncio.get_running_loop().time()
    try:
        async with network.client(ctx, timeout_seconds=route.timeout_seconds) as client:
            sdk = AsyncOpenAI(
                base_url=route.base_url,
                api_key=key,
                http_client=client,
                max_retries=0,
            )
            for parts in rendered:
                raw = await sdk.post(
                    media_path,
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
                records = (response.data,) if isinstance(response.data, EmbeddingData) else response.data
                if (
                    len(records) != 1
                    or len(records[0].embedding) != route.dimensions
                    or not all(math.isfinite(value) for value in records[0].embedding)
                ):
                    raise ValueError("One joint input must produce one vector in its declared space")
                if effective_model is not None and effective_model != response.model:
                    raise DomainError(
                        "embedding_space_conflict",
                        "Provider changed model versions within one embedding request",
                    )
                effective_model = response.model
                vector = records[0].embedding
                if route.normalization == "l2":
                    norm = math.sqrt(sum(value * value for value in vector))
                    vector = tuple(value / norm for value in vector) if norm else vector
                vectors.append(vector)
                usages.append(media_usage_from(response))
    except asyncio.CancelledError:
        raise
    except (DomainError, Unavailable):
        raise
    except Exception as error:
        code = str(getattr(error, "status_code", type(error).__name__))
        raise Unavailable(
            f"Media embedding provider failed ({code})",
            context={"task": task, "provider_code": code},
        ) from error
    assert effective_model is not None
    batch = EmbeddingBatch(
        model=effective_model,
        space=vector_space(route, effective_model),
        dimensions=route.dimensions,
        vectors=tuple(vectors),
    )
    usage = combine_usage(tuple(usages))
    result = generation.create_result(
        output=batch,
        output_type=EmbeddingBatch,
        usage=usage,
        task=task,
        model_id=effective_model,
        prompt_digest=prompt_digest,
        schema_digest=schema_digest,
        input_digest=input_digest,
    )
    if cache_policy == "shared":
        generation.cache_put(cache_key, result)
    generation.record_result(
        ctx,
        task=task,
        model=effective_model,
        protocol=route.protocol,
        parameters=TypeAdapter(dict[str, JsonValue]).validate_python(parameters),
        schema=output_schema,
        response_format=None,
        messages=TypeAdapter(JsonValue).validate_python(input_groups),
        media=TypeAdapter(JsonValue).validate_python(input_groups),
        output=batch,
        producer=result.producer,
        usage=result.usage,
        cache_hit=False,
        elapsed_ms=round((asyncio.get_running_loop().time() - started) * 1000),
    )
    return result
