"""Provider embeddings use the shared typed result cache and HTTP client."""

from __future__ import annotations

import asyncio
import math
import time

from nous_sdk.contracts import EmbeddingBatch
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.model_roles import ModelCachePolicy
from openai import AsyncOpenAI
from pydantic import JsonValue, TypeAdapter
from pydantic_ai.usage import RunUsage

from nous.platform.configuration import EmbeddingRoute
from nous.platform.errors import Unavailable
from nous.platform.network.service import NetworkService

from .embedding import combine_usage, vector_space
from .generation import Generation, ModelResult


def usage_from(response) -> RunUsage:
    usage = response.usage
    if usage is None:
        return RunUsage(requests=1)
    input_tokens = int(usage.prompt_tokens or 0)
    total_tokens = int(usage.total_tokens or input_tokens)
    return RunUsage(
        requests=1,
        input_tokens=input_tokens,
        output_tokens=max(0, total_tokens - input_tokens),
    )


async def embed_text(
    ctx: CallContext,
    texts: tuple[str, ...],
    route: EmbeddingRoute,
    key: str,
    network: NetworkService,
    generation: Generation,
    source_ids: tuple[str, ...],
    *,
    cache_policy: ModelCachePolicy = "none",
) -> ModelResult[EmbeddingBatch]:
    if not texts or any(not text.strip() for text in texts):
        raise ValueError("Text embedding inputs must contain nonempty text")
    output_schema = TypeAdapter(EmbeddingBatch).json_schema()
    task = "embedding"
    prompt_digest = generation.digest({"task": task, "operation": "text_embedding"})
    input_digest = generation.digest({"texts": texts})
    schema_digest = generation.digest(output_schema)
    parameters = {
        "path": "/embeddings",
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
                parameters=TypeAdapter(dict[str, object]).validate_python(parameters),
                schema=output_schema,
                response_format=None,
                messages=[{"role": "input", "content": texts}],
                media=[],
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
    started = time.perf_counter()
    try:
        async with network.client(ctx, timeout_seconds=route.timeout_seconds) as client:
            sdk = AsyncOpenAI(
                base_url=route.base_url,
                api_key=key,
                http_client=client,
                max_retries=0,
            )
            for offset in range(0, len(texts), route.max_batch_items):
                group = texts[offset : offset + route.max_batch_items]
                response = await sdk.embeddings.create(
                    model=route.model,
                    input=list(group),
                    dimensions=route.dimensions,
                    extra_body=route.extra_body,
                )
                records = sorted(response.data, key=lambda item: item.index)
                if [item.index for item in records] != list(range(len(group))):
                    raise ValueError("Provider returned inconsistent embedding indexes")
                group_vectors = [tuple(item.embedding) for item in records]
                if any(
                    len(vector) != route.dimensions
                    or not all(math.isfinite(value) for value in vector)
                    for vector in group_vectors
                ):
                    raise ValueError("Provider returned inconsistent embedding dimensions")
                if route.normalization == "l2":
                    normalized: list[tuple[float, ...]] = []
                    for vector in group_vectors:
                        norm = math.sqrt(sum(value * value for value in vector))
                        normalized.append(tuple(value / norm for value in vector) if norm else vector)
                    group_vectors = normalized
                if effective_model is not None and effective_model != response.model:
                    raise DomainError(
                        "embedding_space_conflict",
                        "Provider changed model versions within one embedding request",
                    )
                effective_model = response.model
                vectors.extend(group_vectors)
                usages.append(usage_from(response))
    except asyncio.CancelledError:
        raise
    except (DomainError, Unavailable):
        raise
    except Exception as error:
        code = str(getattr(error, "status_code", type(error).__name__))
        raise Unavailable(
            f"Text embedding provider failed ({code})",
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
        parameters=TypeAdapter(dict[str, object]).validate_python(parameters),
        schema=output_schema,
        response_format=None,
        messages=[{"role": "input", "content": texts}],
        media=[],
        output=batch,
        producer=result.producer,
        usage=result.usage,
        cache_hit=False,
        elapsed_ms=round((time.perf_counter() - started) * 1000),
    )
    return result
