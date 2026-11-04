"""Complete Wave use cases over immutable snapshots and source-aware query planning."""

from __future__ import annotations

import json
import logging
from collections.abc import Awaitable, Callable, Sequence
from pathlib import Path
from typing import cast

from nous.application.embedding import EmbeddingOperations
from nous.common.operations import record_completed
from nous.content.artifacts import ArtifactStore
from nous.content.results import ResultStore
from nous.media.preparation import MediaPreparer
from nous.runtime.execution import ExecutionManager
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database, WriteTx
from nous.temporal.domains import TimeService

from .contracts import ContextFragment, CorpusView, WaveQuery
from .query import WaveQueries
from .snapshots import WaveSnapshots, encode_json, view_profile

logger = logging.getLogger(__name__)


class WaveOperations:
    """Product operations compose storage, semantic planning and Native calculations."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        paths: ResolvedPaths,
        instance_id: str,
        time: TimeService,
        artifacts: ArtifactStore,
        results: ResultStore,
        embedding: EmbeddingOperations,
        preparer: MediaPreparer,
    ) -> None:
        self.execution = execution
        self.snapshots = WaveSnapshots(
            database, execution, paths, instance_id, time, artifacts, results, embedding, preparer
        )
        self.queries = WaveQueries(execution, artifacts, embedding, time, instance_id)

    def close(self) -> None:
        self.snapshots.close()

    async def status(self) -> dict[str, object]:
        return await self.snapshots.status()

    def profile(self, space: str | None, policy: dict[str, object]) -> str:
        return view_profile(self.execution.snapshot, space, policy)

    async def current(self, collection: str) -> dict[str, object] | None:
        row = await self.snapshots.current(collection)
        if row is None:
            return None
        return {
            "snapshot": {
                "instance_id": self.snapshots.instance_id,
                "id": row["id"],
                "address": row["address"],
            },
            "covered_generation": row["covered_generation"],
            "profile": row["profile"],
            "metadata": row["mapping"]["metadata"],
        }

    async def build(self, manifest_path: Path, space: str | None = None) -> dict[str, object]:
        async with self.execution.operation("wave.build") as context:
            view = await self.snapshots.import_manifest(manifest_path)
            result = await self.snapshots.build(view, space, context.settings)
            snapshot = cast(dict[str, str], result["snapshot"])
            record_completed(snapshot["address"])
            return result

    async def build_view(
        self,
        view: CorpusView,
        space: str | None = None,
        *,
        publish_if: Callable[[WriteTx], Awaitable[bool]] | None = None,
    ) -> dict[str, object]:
        async with self.execution.operation("wave.build_view") as context:
            result = await self.snapshots.build(
                view, space, context.settings, publish_if=publish_if
            )
            record_completed(cast(dict[str, str], result["snapshot"])["address"])
            return result

    async def query(self, request: WaveQuery) -> dict[str, object]:
        async with self.execution.operation("wave.query"):
            row = await self.snapshots.resolve(request.snapshot)
            plan = await self.queries.prepare(request, row)
            snapshot, token = await self.snapshots.open(row)
            result = await self.snapshots.compute(
                lambda: snapshot.search(
                    mode=plan.mode,
                    text=plan.text,
                    vector=plan.vector,
                    ghost_vectors=plan.ghosts,
                    allowed_rows=plan.allowed,
                    options_json=encode_json(plan.options),
                    candidate_pool=plan.pool,
                    cancel=token,
                ),
                token,
            )
            output = await self.queries.materialize(request, row, result, plan.summary)
            logger.info(
                "wave.query.completed",
                extra={
                    "snapshot": row["address"],
                    "mode": plan.mode,
                    "returned": len(cast(list[object], output["hits"])),
                    "lanes": output["used_lanes"],
                },
            )
            return output

    async def read(
        self, ref: str, candidate_key: str, *, sources: bool = False
    ) -> dict[str, object]:
        async with self.execution.operation("wave.read"):
            row = await self.snapshots.resolve(ref)
            return await self.queries.read(row, candidate_key, sources=sources)

    async def neighborhood(self, ref: str, concepts: Sequence[str]) -> dict[str, object]:
        async with self.execution.operation("wave.neighborhood"):
            row = await self.snapshots.resolve(ref)
            rows = self.queries.neighborhood_rows(row, concepts)
            snapshot, token = await self.snapshots.open(row)
            result = await self.snapshots.compute(
                lambda: snapshot.neighborhood(rows, cancel=token), token
            )
            return self.queries.neighborhood(row, concepts, result)

    async def segment(self, ref: str, fragments: Sequence[ContextFragment]) -> dict[str, object]:
        async with self.execution.operation("wave.segment") as context:
            row = await self.snapshots.resolve(ref)
            vectors, keys = await self.queries.segment_vectors(row, fragments)
            if not keys:
                return {"snapshot": row["address"], "segments": []}
            self.snapshots.engine()
            import nous_native as native

            result = await self.execution.blocking(
                native.segment,
                vectors,
                context.settings.typed_values().wave.context.segment_threshold,
            )
            ranges = json.loads(result.ranges_json)
            return {
                "snapshot": row["address"],
                "segments": [
                    {
                        "start": item["start"],
                        "end": item["end"],
                        "fragments": keys[item["start"] : item["end"]],
                    }
                    for item in ranges
                ],
                "dimensions": vectors.shape[1],
            }

    async def inspect(self, ref: str) -> dict[str, object]:
        async with self.execution.operation("wave.inspect"):
            row = await self.snapshots.resolve(ref)
            snapshot, _ = await self.snapshots.open(row)
            return {
                "snapshot": {
                    "instance_id": self.snapshots.instance_id,
                    "id": row["id"],
                    "address": row["address"],
                },
                "collection": row["mapping"]["name"],
                "space": row["space_definition"],
                "identity": row["identity"],
                "covered_generation": row["covered_generation"],
                "profile": row["profile"],
                "metadata": row["mapping"]["metadata"],
                "capabilities": list(snapshot.capabilities),
                "statistics": json.loads(snapshot.statistics_json),
                "source_roots": len(row["mapping"]["roots"]),
                "created_at": row["created_at"],
            }
