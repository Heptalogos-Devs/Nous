"""Embedding batches, canonical adopted rows and temporary query vectors."""

from __future__ import annotations

import hashlib
import mmap
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import numpy as np
import numpy.typing as npt
from pydantic import BaseModel, ConfigDict, JsonValue

from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.common.operations import record_completed
from nous.configuration import ConfigSnapshot
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    InputSpec,
    OutputContent,
    ResultRef,
)
from nous.content.results import ResultStore
from nous.models.contracts import EmbeddingResult
from nous.models.embedding_store import EmbeddingSpaceRegistry, StoredVectorRow
from nous.models.embeddings import EmbeddingExecutor, resolved_space
from nous.models.vector_contracts import Purpose, SpaceDefinition, VectorRef
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database


@dataclass(frozen=True)
class EmbeddingBatchRef:
    space: SpaceDefinition
    purpose: Purpose
    inputs: tuple[ContentSelection, ...]
    rows: tuple[StoredVectorRow, ...]


class EmbeddingOutcome(BaseModel):
    model_config = ConfigDict(frozen=True)
    space: SpaceDefinition
    purpose: Purpose
    rows: tuple[VectorRef, ...]
    results: tuple[ResultRef, ...]
    coverage: dict[str, JsonValue]


class EmbeddingOperations:
    """Produce and reuse exact-input vectors in a configured embedding space.

    Network requests run outside the write transaction. Adoption selects the
    canonical rows in SQL, including when callers concurrently encode the same
    input. All dependencies are borrowed from the runtime.
    """

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

    async def _inputs(
        self, inputs: Sequence[str], texts: Sequence[str], snapshot: ConfigSnapshot, *, adopt: bool
    ) -> tuple[tuple[str, ...], tuple[ContentSelection, ...]]:
        values: list[str] = []
        selections: list[ContentSelection] = []
        limit = snapshot.values.content.text_read_max_bytes
        for item in inputs:
            if adopt:
                artifact = await self.artifacts.accept(
                    InputSpec(kind="artifact", artifact=item)
                    if identifier_type(item) == "art"
                    else InputSpec(kind="file", path=item)
                )
                selections.append(ContentSelection(artifact=artifact.ref))
                continue
            if identifier_type(item) == "art":
                artifact = await self.artifacts.get(item)
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
        for value in texts:
            if len(value.encode("utf-8")) > limit:
                raise NousError("content.too_large", "Input exceeds the configured text read limit")
            values.append(value)
            if adopt:
                artifact = await self.artifacts.ingest(value)
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
        return tuple(
            hashlib.sha256((prefix + value).encode("utf-8")).hexdigest() for value in values
        )

    async def _adopt_encoded(
        self,
        definition: SpaceDefinition,
        purpose: Purpose,
        generated: EmbeddingResult,
        inputs: tuple[ContentSelection, ...],
        digests: tuple[str, ...],
    ) -> dict[str, StoredVectorRow]:
        prepared = await self.results.prepare_outputs(
            {
                "vectors": OutputContent(
                    data=generated.npy, media_type="application/x-npy", display_name="vectors.npy"
                )
            }
        )
        groups = tuple(
            InputGroup(
                key=f"S{index + 1}",
                slot_key=f"S{index + 1}",
                name=f"S{index + 1}",
                parts=(InputPart(content=source, representation="text", media_type="text/plain"),),
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
                parameters={
                    "usage": generated.usage,
                    "attempts": generated.attempts,
                },
                coverage={"inputs": len(inputs), "purpose": purpose},
            )
            output = next(item for item in result.outputs if item.name == "vectors")
            canonical = await self.registry.attach_in(
                tx,
                definition=definition,
                result_id=result.ref.id,
                artifact_id=output.artifact.id,
                purpose=purpose,
                input_digests=digests,
            )
        record_completed(result.ref.identifier)
        return canonical

    async def probe(
        self, model_id: str, inputs: Sequence[str], texts: Sequence[str]
    ) -> EmbeddingResult:
        async with self.execution.operation("embedding.probe") as context:
            values, _ = await self._inputs(inputs, texts, context.settings, adopt=False)
            return await self.executor.probe(model_id, values, context.settings, context=context)

    async def run(
        self, space_id: str, purpose: Purpose, inputs: Sequence[str], texts: Sequence[str]
    ) -> EmbeddingOutcome:
        async with self.execution.operation("embedding.run") as context:
            _, sources = await self._inputs(inputs, texts, context.settings, adopt=True)
            batch = await self.ensure(sources, space_id, purpose)
            results: dict[str, ResultRef] = {}
            artifacts: dict[str, ArtifactRef] = {}
            for row in batch.rows:
                if row.result_id not in results:
                    result = await self.results.get(row.result_id)
                    results[row.result_id] = result.ref
                    artifacts.update({item.artifact.id: item.artifact for item in result.outputs})
            return EmbeddingOutcome(
                space=batch.space,
                purpose=purpose,
                rows=tuple(
                    VectorRef(
                        space=batch.space,
                        purpose=purpose,
                        artifact=artifacts[row.artifact_id],
                        row_index=row.row_index,
                    )
                    for row in batch.rows
                ),
                results=tuple(results.values()),
                coverage={"inputs": len(sources), "rows": len(batch.rows), "batches": len(results)},
            )

    async def ensure(
        self, inputs: Sequence[ContentSelection], space_id: str, purpose: Purpose
    ) -> EmbeddingBatchRef:
        """Ensure vectors for exact selections and return references in input order.

        Reuses adopted rows by space, purpose and exact input; repeated inputs can
        refer to the same row. New batches are adopted as they complete. If a
        later batch fails, ``NousError.details`` reports completed and unfinished
        input indices; earlier adopted batches remain available for a retry.

        Returns:
            An instance-bound batch reference with one row per input in order.

        Raises:
            NousError: ``embedding.empty_input`` for no selections, or an
                execution or adoption error with completed and unfinished indices.
        """
        async with self.execution.operation("embedding.ensure") as context:
            definition = resolved_space(context.settings, space_id)
            sources = tuple(inputs)
            if not sources:
                raise NousError("embedding.empty_input", "Embedding requires at least one input")
            values = tuple([await self.artifacts.read_selection(item) for item in sources])
            digests = self._digests(definition, purpose, values)
            available = await self.registry.find_rows(definition, purpose, digests)
            missing: dict[str, int] = {}
            for index, digest in enumerate(digests):
                if digest not in available:
                    missing.setdefault(digest, index)
            missing_items = list(missing.items())
            completed_indices = {
                index for index, digest in enumerate(digests) if digest in available
            }
            try:
                if missing_items:
                    async for encoded in self.executor.encode_batches(
                        tuple(values[index] for _, index in missing_items),
                        context.settings,
                        space_id=space_id,
                        purpose=purpose,
                        context=context,
                        original_indices=tuple(index for _, index in missing_items),
                    ):
                        chosen = [missing_items[index] for index in encoded.original_indices]
                        original = [index for _, index in chosen]
                        request: dict[str, JsonValue] = {
                            **encoded.result.request,
                            "original_indices": list[JsonValue](original),
                        }
                        generated = EmbeddingResult(
                            encoded.result.vectors,
                            encoded.result.raw_vectors,
                            encoded.result.producer,
                            encoded.result.usage,
                            request,
                            encoded.result.attempts,
                            encoded.result.npy,
                        )
                        canonical = await self._adopt_encoded(
                            definition,
                            purpose,
                            generated,
                            tuple(sources[index] for index in original),
                            tuple(digest for digest, _ in chosen),
                        )
                        available.update(canonical)
                        completed_indices.update(
                            index for index, digest in enumerate(digests) if digest in canonical
                        )
            except NousError as error:
                error.details["completed_indices"] = list[JsonValue](sorted(completed_indices))
                error.details["unfinished_indices"] = list[JsonValue](
                    [index for index in range(len(sources)) if index not in completed_indices]
                )
                raise
            return EmbeddingBatchRef(
                definition, purpose, sources, tuple(available[digest] for digest in digests)
            )

    async def encode_query(
        self, inputs: Sequence[str | ContentSelection], space_id: str
    ) -> npt.NDArray[np.float32]:
        """Encode temporary query strings or exact content selections in order.

        Returns a float32 matrix with one row per input and the configured space
        dimensions. Empty input yields an empty matrix of that width. Query
        vectors are returned to the caller without adopting persistent rows.
        """
        async with self.execution.operation("embedding.query") as context:
            definition = resolved_space(context.settings, space_id)
            values = tuple(
                [
                    await self.artifacts.read_selection(value)
                    if isinstance(value, ContentSelection)
                    else value
                    for value in inputs
                ]
            )
            output = np.empty((len(values), definition.dimensions), dtype=np.float32)
            if values:
                async for encoded in self.executor.encode_batches(
                    values, context.settings, space_id=space_id, purpose="query", context=context
                ):
                    output[list(encoded.original_indices)] = np.asarray(
                        encoded.result.vectors, dtype=np.float32
                    )
            return output

    @staticmethod
    def _copy_rows(
        path: Path,
        locations: list[tuple[int, int]],
        output: npt.NDArray[np.float32],
        dimensions: int,
    ) -> None:
        try:
            loaded: object = np.load(path, mmap_mode="r", allow_pickle=False)
        except (OSError, ValueError) as error:
            raise NousError(
                "embedding.corrupt", "Stored vector artifact cannot be mapped"
            ) from error
        if not isinstance(loaded, np.ndarray):
            raise NousError("embedding.corrupt", "Stored vectors must be an NPY matrix")
        array = cast(npt.NDArray[np.float32], loaded)
        try:
            if array.dtype != np.float32 or array.ndim != 2 or array.shape[1] != dimensions:
                raise NousError("embedding.corrupt", "Stored vector matrix has invalid shape")
            for target, source in locations:
                if not 0 <= source < array.shape[0]:
                    raise NousError("embedding.corrupt", "Vector row is out of range")
                vector = array[source]
                if not np.isfinite(vector).all():
                    raise NousError("embedding.corrupt", "Vector row contains non-finite values")
                output[target] = vector
                del vector
        finally:
            mapping = getattr(array, "_mmap", None)
            if isinstance(mapping, mmap.mmap):
                mapping.close()

    async def _read_rows(
        self, definition: SpaceDefinition, rows: Sequence[StoredVectorRow]
    ) -> npt.NDArray[np.float32]:
        output = np.empty((len(rows), definition.dimensions), dtype=np.float32)
        grouped: dict[str, list[tuple[int, int]]] = {}
        for index, row in enumerate(rows):
            grouped.setdefault(row.artifact_id, []).append((index, row.row_index))
        for artifact_id, locations in grouped.items():
            async with self.artifacts.borrow_local(artifact_id) as path:
                await self.execution.blocking(
                    self._copy_rows, path, locations, output, definition.dimensions
                )
        return output

    async def read(self, batch: EmbeddingBatchRef) -> npt.NDArray[np.float32]:
        return await self._read_rows(batch.space, batch.rows)

    async def read_vector(self, ref: VectorRef) -> npt.NDArray[np.float32]:
        """Read a copied canonical vector identified by an instance-bound reference.

        A reference from another instance or a corrupt stored row raises
        ``NousError``. The returned array does not retain the mapped file.
        """
        if ref.artifact.instance_id != self.artifacts.instance_id:
            raise NousError("content.instance_mismatch", "Vector belongs to another instance")
        row = await self.registry.resolve_row(
            ref.space, ref.purpose, ref.artifact.id, ref.row_index
        )
        return (await self._read_rows(ref.space, (row,)))[0]
