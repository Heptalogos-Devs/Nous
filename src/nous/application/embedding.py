"""Text embedding operations and atomic adoption into a bound space."""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import Literal

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.content.contracts import OutputContent, ProcessingResult, ResultInput
from nous.content.store import ArtifactStore, ResultStore
from nous.models.contracts import EmbeddingResult
from nous.models.embedding_store import EmbeddingSpaceRegistry
from nous.models.embeddings import EmbeddingExecutor, resolved_space
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database


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

    async def _inputs(
        self,
        inputs: Sequence[str],
        texts: Sequence[str],
        snapshot: ConfigSnapshot,
        *,
        adopt: bool,
    ) -> tuple[tuple[str, ...], tuple[ResultInput, ...]]:
        values: list[str] = []
        sources: list[ResultInput] = []
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
                sources.append(ResultInput(artifact=artifact.ref, key=f"S{len(values)}"))
        for text in texts:
            if len(text.encode("utf-8")) > limit:
                raise NousError("content.too_large", "Input exceeds the configured text read limit")
            values.append(text)
            if adopt:
                artifact = await self.artifacts.ingest(text)
                sources.append(ResultInput(artifact=artifact.ref, key=f"S{len(values)}"))
        return tuple(values), tuple(sources)

    async def probe(
        self,
        model_id: str,
        inputs: Sequence[str],
        texts: Sequence[str],
    ) -> EmbeddingResult:
        async with self.execution.operation("embedding.probe") as context:
            values, _ = await self._inputs(inputs, texts, context.settings, adopt=False)
            return await self.executor.probe(model_id, values, context.settings, context=context)

    async def run(
        self,
        space_id: str,
        purpose: Literal["query", "document"],
        inputs: Sequence[str],
        texts: Sequence[str],
    ) -> ProcessingResult:
        async with self.execution.operation("embedding.run") as context:
            snapshot = context.settings
            values, sources = await self._inputs(inputs, texts, snapshot, adopt=True)
            definition = resolved_space(snapshot, space_id)
            await self.registry.check(definition)
            generated = await self.executor.run(
                space_id,
                values,
                purpose,
                snapshot,
                context=context,
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
            async with self.database.write() as tx:
                await self.registry.bind_in(tx, definition)
                result = await self.results.adopt_in(
                    tx,
                    operation="embedding",
                    producer=generated.producer,
                    inputs=sources,
                    prepared_outputs=prepared,
                    request=generated.request,
                    parameters={"usage": generated.usage, "attempts": generated.attempts},
                    coverage={"inputs": len(values), "purpose": purpose},
                )
                await self.registry.attach_in(
                    tx,
                    definition=definition,
                    result_id=result.ref.id,
                    artifact_id=result.outputs[0].artifact.id,
                    purpose=purpose,
                    input_count=len(values),
                )
            return result
