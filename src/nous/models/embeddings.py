"""Explicit text embedding contracts and validated provider execution."""

from __future__ import annotations

import asyncio
import io
import math
from typing import TYPE_CHECKING, Literal, cast

import numpy as np
from openai import APIConnectionError, APIStatusError, omit
from pydantic import JsonValue

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.configuration.metadata import ConfigModel

from .configuration import EncodingConfig
from .contracts import EmbeddingResult
from .executor import model_error, retry_delay, retryable, stage_deadline

if TYPE_CHECKING:
    from nous.network import ModelConnections
    from nous.runtime.execution import OperationContext


class SpaceDefinition(ConfigModel):
    """Vector semantics used for binding comparisons, independent of connection resource IDs."""

    space_id: str
    model_name: str
    deployment_revision: str
    dimensions: int
    modalities: list[Literal["text", "image", "audio", "video"]]
    normalization: Literal["none", "l2"]
    encoding: EncodingConfig


def resolved_space(snapshot: ConfigSnapshot, space_id: str) -> SpaceDefinition:
    """Resolve declared model identity and encoding rules from the same configuration snapshot."""
    settings = snapshot.typed_values()
    space = settings.embedding.spaces.get(space_id)
    if space is None:
        raise NousError("embedding.space_unknown", f"Embedding space is not configured: {space_id}")
    model = settings.models[space.model]
    if model.deployment_revision and model.deployment_revision != space.deployment_revision:
        raise NousError(
            "embedding.deployment_conflict", "Space and model deployment revisions differ"
        )
    return SpaceDefinition(
        space_id=space_id,
        model_name=model.model_name,
        deployment_revision=space.deployment_revision,
        dimensions=space.dimensions,
        modalities=space.modalities,
        normalization=space.normalization,
        encoding=space.encoding,
    )


class EmbeddingExecutor:
    """Produce validated vector bytes; the application binds their space during adoption."""

    def __init__(self, connections: ModelConnections) -> None:
        self.connections = connections

    async def probe(
        self,
        model_id: str,
        texts: tuple[str, ...],
        snapshot: ConfigSnapshot,
        *,
        context: OperationContext | None = None,
    ) -> EmbeddingResult:
        """Measure a model's actual output shape without creating a formal space binding."""
        return await self._execute(model_id, texts, snapshot, context=context)

    async def run(
        self,
        space_id: str,
        texts: tuple[str, ...],
        purpose: Literal["query", "document"],
        snapshot: ConfigSnapshot,
        *,
        context: OperationContext | None = None,
    ) -> EmbeddingResult:
        """Apply the selected space's purpose prefix, dimensions and normalization together."""
        space = resolved_space(snapshot, space_id)
        if space.modalities != ["text"] or space.encoding.input_combination != "text":
            raise NousError(
                "embedding.modality_unsupported", "This operation accepts a text embedding space"
            )
        prefix = (
            space.encoding.query_instruction
            if purpose == "query"
            else space.encoding.document_instruction
        )
        actual = tuple(prefix + text for text in texts)
        model_id = snapshot.typed_values().embedding.spaces[space_id].model
        result = await self._execute(
            model_id,
            actual,
            snapshot,
            dimensions=space.dimensions,
            normalization=space.normalization,
            context=context,
        )
        return EmbeddingResult(
            result.vectors,
            result.raw_vectors,
            result.producer,
            result.usage,
            {
                **result.request,
                "space": space.model_dump(mode="json"),
                "space_id": space_id,
                "purpose": purpose,
                "original_texts": list(texts),
            },
            result.attempts,
            result.npy,
        )

    async def _execute(
        self,
        model_id: str,
        texts: tuple[str, ...],
        snapshot: ConfigSnapshot,
        *,
        dimensions: int | None = None,
        normalization: Literal["none", "l2"] = "none",
        context: OperationContext | None = None,
    ) -> EmbeddingResult:
        if not texts or any(not text.strip() for text in texts):
            raise NousError("embedding.empty_input", "Embedding requires nonempty text items")
        settings = snapshot.typed_values()
        model = settings.models.get(model_id)
        if model is None or "embedding" not in model.capabilities:
            raise NousError(
                "embedding.model_unavailable",
                "The selected model does not declare embedding capability",
            )
        if settings.providers[model.provider].protocol != "openai-chat-completions":
            raise NousError(
                "model.protocol_unsupported",
                "Embedding requires the configured compatible protocol",
            )
        limit = settings.model_execution
        estimate = math.ceil(sum(len(text.encode("utf-8")) for text in texts) / 3)
        if estimate > limit.max_input_tokens:
            raise NousError(
                "embedding.input_limit",
                "Embedding text exceeds the input allowance",
                details={"estimated_input_tokens": estimate, "count_kind": "estimated"},
            )
        async with asyncio.timeout_at(stage_deadline(limit.timeout_seconds, context)):
            for attempt in range(1, limit.max_attempts + 1):
                try:
                    response = await self.connections.client(model.provider).embeddings.create(
                        model=model.model_name,
                        input=list(texts),
                        encoding_format="float",
                        dimensions=dimensions if dimensions is not None else omit,
                        timeout=self.connections.timeout(model.provider, snapshot),
                    )
                    items = sorted(response.data, key=lambda item: item.index)
                    # Provider batch order can differ; indexes must still cover every input exactly.
                    if [item.index for item in items] != list(range(len(texts))):
                        raise NousError(
                            "embedding.batch_mismatch",
                            "Returned vectors do not match the input order",
                        )
                    raw = tuple(tuple(float(value) for value in item.embedding) for item in items)
                    actual_dimensions = len(raw[0]) if raw else 0
                    if actual_dimensions == 0 or (
                        dimensions is not None and actual_dimensions != dimensions
                    ):
                        raise NousError(
                            "embedding.dimension_mismatch",
                            "Returned vector dimensions differ from the space",
                        )
                    if any(
                        len(vector) != actual_dimensions
                        or not all(math.isfinite(value) for value in vector)
                        for vector in raw
                    ):
                        raise NousError(
                            "embedding.invalid_vector",
                            "Returned vectors must have consistent dimensions and finite values",
                        )
                    matrix = np.asarray(raw, dtype=np.float32)
                    # Conversion itself can overflow even when the original values are finite.
                    if not np.isfinite(matrix).all():
                        raise NousError(
                            "embedding.invalid_vector",
                            "Embedding values exceed finite float32 range",
                        )
                    if normalization == "l2":
                        norms = np.linalg.norm(matrix.astype(np.float64), axis=1, keepdims=True)
                        if np.any(norms == 0):
                            raise NousError(
                                "embedding.zero_norm", "A zero vector cannot be l2 normalized"
                            )
                        matrix = (matrix / norms).astype(np.float32)
                    output = io.BytesIO()
                    np.save(output, matrix, allow_pickle=False)
                    vectors = tuple(tuple(float(value) for value in row) for row in matrix)
                    return EmbeddingResult(
                        vectors,
                        raw,
                        {
                            "kind": "model",
                            "provider": model.provider,
                            "protocol": "openai-chat-completions",
                            "model_id": model_id,
                            "requested_model_name": model.model_name,
                            "returned_model_name": response.model,
                            "deployment_revision": model.deployment_revision,
                        },
                        cast(dict[str, JsonValue], response.usage.model_dump(mode="json")),
                        {
                            "model_id": model_id,
                            "texts": list(texts),
                            "dimensions": actual_dimensions,
                            "normalization": normalization,
                            "dtype": "float32",
                            "estimated_input_tokens": estimate,
                            "count_kind": "estimated",
                        },
                        attempt,
                        output.getvalue(),
                    )
                except (APIConnectionError, APIStatusError) as error:
                    if attempt >= limit.max_attempts or not retryable(error):
                        raise model_error(error, attempt) from error
                    await retry_delay(error)
        raise AssertionError("attempt limit must be positive")
