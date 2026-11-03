"""Text embeddings, exact adopted-row reuse, and durable space identity."""

from __future__ import annotations

import asyncio
import hashlib
import io
import json
import math
from collections import deque
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, cast

import numpy as np
import numpy.typing as npt
from pydantic import JsonValue

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.content.contracts import (
    ContentSelection,
    InputGroup,
    InputPart,
    OutputContent,
    ProcessingResult,
)
from nous.content.store import ArtifactStore, ResultStore
from nous.models.contracts import EmbeddingResult
from nous.models.embedding_store import EmbeddingSpaceRegistry, StoredVectorRow
from nous.models.embeddings import EmbeddingExecutor, SpaceDefinition, resolved_space
from nous.runtime.execution import ExecutionManager, OperationContext
from nous.storage.database import Database

Purpose = Literal["query", "document"]


@dataclass(frozen=True)
class EmbeddingBatchRef:
    """Ordered input selections and their rows in adopted immutable vector artifacts."""

    space: SpaceDefinition
    purpose: Purpose
    inputs: tuple[ContentSelection, ...]
    rows: tuple[StoredVectorRow, ...]


class EmbeddingOperations:
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        artifacts: ArtifactStore,
        results: ResultStore,
        registry: EmbeddingSpaceRegistry,
        executor: EmbeddingExecutor,
    ) -> None:
        self.database, self.execution, self.artifacts = database, execution, artifacts
        self.results, self.registry, self.executor = results, registry, executor
        self._ensure_lock = asyncio.Lock()

    async def _inputs(
        self,
        inputs: Sequence[str],
        texts: Sequence[str],
        snapshot: ConfigSnapshot,
        *,
        adopt: bool,
    ) -> tuple[tuple[str, ...], tuple[ContentSelection, ...]]:
        values: list[str] = []
        selections: list[ContentSelection] = []
        limit = snapshot.typed_values().content.text_read_max_bytes
        for item in inputs:
            if item.startswith("art:"):
                artifact = await self.artifacts.get(item)
                value = await self.artifacts.read_text(artifact.ref, max_bytes=limit)
            elif adopt:
                artifact = await self.artifacts.ingest(Path(item))
                value = await self.artifacts.read_text(artifact.ref, max_bytes=limit)
            else:
                path = Path(item)
                if (await self.execution.blocking(path.stat)).st_size > limit:
                    raise NousError(
                        "content.too_large", "Input exceeds the configured text read limit"
                    )
                value = await self.execution.blocking(path.read_text, encoding="utf-8")
                artifact = None
            values.append(value)
            if artifact is not None:
                selections.append(ContentSelection(artifact=artifact.ref))
        for text in texts:
            if len(text.encode("utf-8")) > limit:
                raise NousError("content.too_large", "Input exceeds the configured text read limit")
            values.append(text)
            if adopt:
                artifact = await self.artifacts.ingest(text)
                selections.append(ContentSelection(artifact=artifact.ref))
        return tuple(values), tuple(selections)

    @staticmethod
    def _digests(
        definition: SpaceDefinition, purpose: Purpose, values: Sequence[str]
    ) -> tuple[str, ...]:
        prefix = (
            definition.encoding.query_instruction
            if purpose == "query"
            else definition.encoding.document_instruction
        )
        return tuple(hashlib.sha256((prefix + text).encode("utf-8")).hexdigest() for text in values)

    async def _adopt(
        self,
        definition: SpaceDefinition,
        purpose: Purpose,
        values: tuple[str, ...],
        inputs: tuple[ContentSelection, ...],
        context: OperationContext,
    ) -> tuple[ProcessingResult, tuple[StoredVectorRow, ...]]:
        generated = await self.executor.run(
            definition.space_id, values, purpose, context.settings, context=context
        )
        prepared = await self.results.prepare_outputs(
            {
                "vectors": OutputContent(
                    data=generated.npy,
                    media_type="application/x-npy",
                    display_name="vectors.npy",
                )
            }
        )
        groups = tuple(
            InputGroup(
                key=f"S{index + 1}",
                name=f"S{index + 1}",
                parts=(InputPart(content=source, representation="text"),),
            )
            for index, source in enumerate(inputs)
        )
        async with self.database.write() as tx:
            await self.registry.bind_in(tx, definition)
            result = await self.results.adopt_in(
                tx,
                operation="embedding",
                producer=generated.producer,
                inputs=groups,
                prepared_outputs=prepared,
                request=generated.request,
                parameters={"usage": generated.usage, "attempts": generated.attempts},
                coverage={"inputs": len(values), "purpose": purpose},
            )
            output = next(item for item in result.outputs if item.name == "vectors")
            await self.registry.attach_in(
                tx,
                definition=definition,
                result_id=result.ref.id,
                artifact_id=output.artifact.id,
                purpose=purpose,
                input_digests=self._digests(definition, purpose, values),
            )
        return result, tuple(
            StoredVectorRow(result.ref.id, output.artifact.id, row) for row in range(len(values))
        )

    async def probe(
        self, model_id: str, inputs: Sequence[str], texts: Sequence[str]
    ) -> EmbeddingResult:
        async with self.execution.operation("embedding.probe") as context:
            values, _ = await self._inputs(inputs, texts, context.settings, adopt=False)
            return await self.executor.probe(model_id, values, context.settings, context=context)

    async def run(
        self, space_id: str, purpose: Purpose, inputs: Sequence[str], texts: Sequence[str]
    ) -> ProcessingResult:
        async with self.execution.operation("embedding.run") as context:
            values, sources = await self._inputs(inputs, texts, context.settings, adopt=True)
            definition = resolved_space(context.settings, space_id)
            await self.registry.check(definition)
            result, _ = await self._adopt(definition, purpose, values, sources, context)
            return result

    async def ensure(
        self, inputs: Sequence[ContentSelection], space_id: str, purpose: Purpose
    ) -> EmbeddingBatchRef:
        """Adopt missing exact inputs in bounded batches and reuse previously adopted rows."""
        async with self.execution.operation("embedding.ensure") as context:
            definition = resolved_space(context.settings, space_id)
            limit = context.settings.typed_values().content.text_read_max_bytes
            selections = tuple(inputs)
            values = tuple([await self.artifacts.read_selection(item) for item in selections])
            if any(len(value.encode("utf-8")) > limit for value in values):
                raise NousError("content.too_large", "Input exceeds the configured text read limit")
            if any(not value.strip() for value in values):
                raise NousError("embedding.empty_input", "Embedding requires nonempty text items")
            digests = self._digests(definition, purpose, values)
            # Serializing adoption prevents concurrent requests from buying the same missing row.
            async with self._ensure_lock:
                available = await self.registry.find_rows(definition, purpose, digests)
                missing: dict[str, int] = {}
                for index, digest in enumerate(digests):
                    if digest not in available:
                        missing.setdefault(digest, index)
                remaining = deque(missing.items())
                settings = context.settings.typed_values()
                max_items = settings.embedding.max_batch_size
                max_tokens = settings.model_execution.max_input_tokens
                prefix = (
                    definition.encoding.query_instruction
                    if purpose == "query"
                    else definition.encoding.document_instruction
                )
                completed: list[str] = []
                try:
                    while remaining:
                        batch: list[tuple[str, int]] = []
                        size = 0
                        while remaining and len(batch) < max_items:
                            digest, index = remaining[0]
                            added = len((prefix + values[index]).encode("utf-8"))
                            if batch and math.ceil((size + added) / 3) > max_tokens:
                                break
                            batch.append(remaining.popleft())
                            size += added
                        result, rows = await self._adopt(
                            definition,
                            purpose,
                            tuple(values[index] for _, index in batch),
                            tuple(selections[index] for _, index in batch),
                            context,
                        )
                        completed.append(result.ref.address)
                        available.update(
                            {digest: row for (digest, _), row in zip(batch, rows, strict=True)}
                        )
                except BaseException as error:
                    if completed:
                        if isinstance(error, NousError):
                            error.details["completed_result_refs"] = list[JsonValue](completed)
                        elif isinstance(
                            error, (asyncio.CancelledError, TimeoutError, KeyboardInterrupt)
                        ):
                            error.add_note(json.dumps({"completed_result_refs": completed}))
                        elif isinstance(error, Exception):
                            raise NousError(
                                "embedding.partial_failure",
                                "Embedding stopped after adopting earlier batches",
                                details={"completed_result_refs": completed},
                                cause=error,
                            ) from error
                    raise
                return EmbeddingBatchRef(
                    space=definition,
                    purpose=purpose,
                    inputs=selections,
                    rows=tuple(available[digest] for digest in digests),
                )

    async def read(self, batch: EmbeddingBatchRef) -> npt.NDArray[np.float32]:
        """Materialize the exact ordered matrix without contacting the model."""
        matrices: dict[str, npt.NDArray[np.float32]] = {}
        output = np.empty((len(batch.rows), batch.space.dimensions), dtype=np.float32)
        for index, row in enumerate(batch.rows):
            matrix = matrices.get(row.artifact_id)
            if matrix is None:
                blob = await self.artifacts.blob(row.artifact_id)
                data = await self.artifacts.read_bytes(row.artifact_id, max_bytes=blob.byte_length)

                def decode(data: bytes, dimensions: int) -> npt.NDArray[np.float32]:
                    try:
                        loaded: object = np.load(io.BytesIO(data), allow_pickle=False)
                    except (OSError, ValueError) as error:
                        raise NousError(
                            "embedding.corrupt", "Stored vector matrix is invalid"
                        ) from error
                    if not isinstance(loaded, np.ndarray):
                        raise NousError("embedding.corrupt", "Stored vectors must be an NPY matrix")
                    array = cast(npt.NDArray[np.float32], loaded)
                    if (
                        array.dtype != np.float32
                        or array.ndim != 2
                        or array.shape[1] != dimensions
                        or not np.isfinite(array).all()
                    ):
                        raise NousError(
                            "embedding.corrupt", "Stored vector matrix has invalid shape or values"
                        )
                    return array

                matrix = await self.execution.blocking(decode, data, batch.space.dimensions)
                matrices[row.artifact_id] = matrix
            if row.row_index < 0 or row.row_index >= len(matrix):
                raise NousError("embedding.corrupt", "Stored vector row is out of range")
            output[index] = matrix[row.row_index]
        return output
