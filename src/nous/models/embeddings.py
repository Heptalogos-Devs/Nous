"""Explicit text embedding contracts and validated provider execution."""

from __future__ import annotations

import asyncio
import io
import json
import math
from collections.abc import AsyncGenerator, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, cast

import numpy as np
from openai import APIConnectionError, APIResponseValidationError, APIStatusError, omit
from pydantic import BaseModel, FiniteFloat, JsonValue, StrictInt, TypeAdapter, ValidationError

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.configuration.metadata import ConfigModel

from .configuration import EncodingConfig
from .contracts import EmbeddingResult
from .policy import (
    attempt_reports,
    attempt_scope,
    error_report,
    model_error,
    retry_delay,
    retryable,
    stage_deadline,
)

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
    settings = snapshot.values
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
        modalities=list(space.modalities),
        normalization=space.normalization,
        encoding=space.encoding,
    )


@dataclass(frozen=True)
class EncodedBatch:
    original_indices: tuple[int, ...]
    result: EmbeddingResult


class EmbeddingExecutor:
    """Produce validated vector bytes; the application binds their space during adoption."""

    def __init__(self, connections: ModelConnections) -> None:
        self.connections = connections

    async def encode_batches(
        self,
        texts: tuple[str, ...],
        snapshot: ConfigSnapshot,
        *,
        model_id: str | None = None,
        space_id: str | None = None,
        purpose: Literal["query", "document"] = "document",
        context: OperationContext | None = None,
        original_indices: Sequence[int] | None = None,
    ) -> AsyncGenerator[EncodedBatch]:
        """One ordered request planner for probe, adopted vectors and temporary queries."""
        settings = snapshot.values
        definition = resolved_space(snapshot, space_id) if space_id is not None else None
        prefix = ""
        if definition is not None:
            if definition.modalities != ["text"] or definition.encoding.input_combination != "text":
                raise NousError(
                    "embedding.modality_unsupported",
                    "This operation accepts a text embedding space",
                )
            prefix = (
                definition.encoding.query_instruction
                if purpose == "query"
                else definition.encoding.document_instruction
            )
            model_id = settings.embedding.spaces[definition.space_id].model
        if model_id is None or model_id not in settings.models:
            raise NousError("embedding.model_unavailable", "Choose a configured embedding resource")
        model = settings.models[model_id]
        allowance = min(
            settings.model_execution.max_input_tokens,
            model.context_window or settings.model_execution.max_input_tokens,
        )
        remaining = list(range(len(texts)))
        completed: list[int] = []
        index_map = (
            tuple(original_indices) if original_indices is not None else tuple(range(len(texts)))
        )
        if len(index_map) != len(texts):
            raise NousError(
                "embedding.batch_mismatch", "Input indices do not match the encoding inputs"
            )
        while remaining:
            indices: list[int] = []
            size = 0
            while remaining and len(indices) < settings.embedding.max_batch_size:
                index = remaining[0]
                actual = prefix + texts[index]
                added = len(actual.encode("utf-8"))
                if indices and (not texts[index].strip() or math.ceil(added / 3) > allowance):
                    break
                if not texts[index].strip():
                    raise NousError(
                        "embedding.empty_input",
                        "Embedding requires nonempty input",
                        details={
                            "input_index": index_map[index],
                            "unfinished_indices": [index_map[item] for item in remaining],
                        },
                    )
                if math.ceil(added / 3) > allowance:
                    raise NousError(
                        "embedding.input_limit",
                        "One embedding input exceeds the configured allowance",
                        details={
                            "input_index": index_map[index],
                            "completed_indices": [index_map[item] for item in completed],
                            "unfinished_indices": [index_map[item] for item in remaining],
                            "estimated_input_tokens": math.ceil(added / 3),
                            "count_kind": "estimated",
                        },
                    )
                if indices and math.ceil((size + added) / 3) > allowance:
                    break
                indices.append(remaining.pop(0))
                size += added
            try:
                report_start = len(attempt_reports())
                result = await self._execute(
                    model_id,
                    tuple(prefix + texts[index] for index in indices),
                    snapshot,
                    dimensions=definition.dimensions if definition else None,
                    normalization=definition.normalization if definition else "none",
                    context=context,
                    original_indices=tuple(index_map[index] for index in indices),
                )
            except NousError as error:
                error.details["completed_indices"] = list[JsonValue](
                    index_map[item] for item in completed
                )
                error.details["unfinished_indices"] = list[JsonValue](
                    index_map[item] for item in [*indices, *remaining]
                )
                raise
            request: dict[str, JsonValue] = {
                **result.request,
                "original_indices": list[JsonValue](index_map[index] for index in indices),
                "original_texts": list[JsonValue](texts[index] for index in indices),
                "purpose": purpose,
                "attempt_reports": list[JsonValue](attempt_reports(report_start)),
            }
            if definition is not None:
                request.update(
                    {"space": definition.model_dump(mode="json"), "space_id": definition.space_id}
                )
            result = EmbeddingResult(
                result.vectors,
                result.raw_vectors,
                result.producer,
                result.usage,
                request,
                result.attempts,
                result.npy,
            )
            completed.extend(indices)
            yield EncodedBatch(tuple(indices), result)
        if not texts:
            raise NousError("embedding.empty_input", "Embedding requires nonempty input")

    async def probe(
        self,
        model_id: str,
        texts: tuple[str, ...],
        snapshot: ConfigSnapshot,
        *,
        context: OperationContext | None = None,
    ) -> EmbeddingResult:
        report_start = len(attempt_reports())
        batches = [
            batch
            async for batch in self.encode_batches(
                texts, snapshot, model_id=model_id, context=context
            )
        ]
        vectors = tuple(vector for batch in batches for vector in batch.result.vectors)
        if len({batch.result.dimensions for batch in batches}) != 1:
            raise NousError(
                "embedding.dimension_mismatch", "Probe batches returned different vector dimensions"
            )
        raw = tuple(vector for batch in batches for vector in batch.result.raw_vectors)
        usage: dict[str, JsonValue] = {}
        for batch in batches:
            for key, value in batch.result.usage.items():
                if isinstance(value, (int, float)) and not isinstance(value, bool):
                    previous = usage.get(key, 0)
                    usage[key] = (
                        previous
                        if isinstance(previous, (int, float)) and not isinstance(previous, bool)
                        else 0
                    ) + value
        if any(not batch.result.usage for batch in batches):
            usage["coverage"] = "partial" if usage else "unknown"
        output = io.BytesIO()
        np.save(output, np.asarray(vectors, dtype=np.float32), allow_pickle=False)
        return EmbeddingResult(
            vectors,
            raw,
            batches[-1].result.producer,
            usage,
            {
                "model_id": model_id,
                "batches": list[JsonValue](batch.result.request for batch in batches),
                "attempt_reports": list[JsonValue](attempt_reports(report_start)),
            },
            sum(batch.result.attempts for batch in batches),
            output.getvalue(),
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
        original_indices: tuple[int, ...] = (),
    ) -> EmbeddingResult:
        if not texts or any(not text.strip() for text in texts):
            raise NousError("embedding.empty_input", "Embedding requires nonempty text items")
        settings = snapshot.values
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
                with attempt_scope(
                    attempt,
                    {
                        "model_id": model_id,
                        "provider": model.provider,
                        "protocol": "openai-chat-completions",
                        "input_count": len(texts),
                        "input_indices": list[JsonValue](original_indices),
                    },
                    "embedding",
                ) as scope:
                    try:
                        response = await self.connections.client(model.provider).embeddings.create(
                            model=model.model_name,
                            input=list(texts),
                            encoding_format="float",
                            dimensions=dimensions if dimensions is not None else omit,
                            timeout=self.connections.timeout(model.provider, snapshot),
                        )
                        data: object = getattr(response, "data", None)
                        if not isinstance(data, list):
                            raise NousError(
                                "embedding.invalid_response",
                                "Embedding response data must be an array",
                                details={"attempts": attempt},
                            )
                        rows = TypeAdapter(
                            list[tuple[StrictInt, list[FiniteFloat]]]
                        ).validate_python(
                            [
                                (getattr(item, "index", None), getattr(item, "embedding", None))
                                for item in cast(list[object], data)
                            ],
                            strict=True,
                        )
                        rows.sort(key=lambda item: item[0])
                        # Provider batch order can differ; indexes must still cover every input exactly.
                        if [item[0] for item in rows] != list(range(len(texts))):
                            raise NousError(
                                "embedding.batch_mismatch",
                                "Returned vectors do not match the input order",
                            )
                        raw = tuple(tuple(item[1]) for item in rows)
                        actual_dimensions = len(raw[0]) if raw else 0
                        if actual_dimensions == 0 or (
                            dimensions is not None and actual_dimensions != dimensions
                        ):
                            raise NousError(
                                "embedding.dimension_mismatch",
                                "Returned vector dimensions differ from the space",
                            )
                        if any(len(vector) != actual_dimensions for vector in raw):
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
                        returned_model = TypeAdapter(str | None).validate_python(
                            getattr(response, "model", None), strict=True
                        )
                        reported_usage: object = getattr(response, "usage", None)
                        if reported_usage is None:
                            usage: dict[str, JsonValue] = {}
                        elif isinstance(reported_usage, BaseModel):
                            usage = TypeAdapter(dict[str, JsonValue]).validate_python(
                                reported_usage.model_dump(mode="json")
                            )
                        else:
                            raise NousError(
                                "embedding.invalid_response",
                                "Embedding usage must be an object when present",
                                details={"attempts": attempt},
                            )
                        scope.complete(
                            outcome="success",
                            provider_request_id=getattr(response, "_request_id", None),
                            reported_usage=usage,
                        )
                        return EmbeddingResult(
                            vectors,
                            raw,
                            {
                                "kind": "model",
                                "provider": model.provider,
                                "protocol": "openai-chat-completions",
                                "model_id": model_id,
                                "requested_model_name": model.model_name,
                                "returned_model_name": returned_model,
                                "deployment_revision": model.deployment_revision,
                            },
                            usage,
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
                        scope.complete(
                            **error_report(
                                error,
                                next_action="retry_same_route"
                                if attempt < limit.max_attempts and retryable(error)
                                else "raise",
                            )
                        )
                        if attempt >= limit.max_attempts or not retryable(error):
                            raise model_error(error, attempt) from error
                        await retry_delay(error)
                    except (
                        APIResponseValidationError,
                        ValidationError,
                        json.JSONDecodeError,
                    ) as error:
                        scope.complete(**error_report(error))
                        problem = "The gateway returned an invalid embedding response"
                        if isinstance(error, ValidationError):
                            problem = error.errors(include_input=False, include_url=False)[0]["msg"]
                        raise NousError(
                            "embedding.invalid_response",
                            problem,
                            details={"attempts": attempt, "error_type": type(error).__name__},
                            cause=error,
                        ) from error
        raise AssertionError("attempt limit must be positive")
