"""Complete Wave use cases over immutable snapshots and source-aware query planning."""

from __future__ import annotations

import json
import logging
from collections.abc import Awaitable, Callable, Sequence
from pathlib import Path
from typing import cast

from pydantic import TypeAdapter

from nous.application.embedding import EmbeddingOperations
from nous.common.operations import record_completed
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection
from nous.content.results import ResultStore
from nous.media.preparation import MediaPreparer
from nous.runtime.execution import ExecutionManager
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database, WriteTx
from nous.temporal.domains import TimeService

from .contracts import (
    CandidateDescriptor,
    CandidateRef,
    ContextFragment,
    CorpusView,
    SnapshotDescriptor,
    SnapshotOwner,
    SnapshotRef,
    WaveQuery,
)
from .query import WaveQueries
from .snapshots import WaveSnapshots, encode_json, view_profile

logger = logging.getLogger(__name__)


class WaveOperations:
    """Build and query immutable Wave snapshots for an instance.

    Dependencies are borrowed from the runtime, which closes the underlying
    Native resources after operations finish. Manifest imports and typed corpus
    views use the same snapshot builder; queries resolve a particular snapshot.
    """

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

    def profile(
        self,
        space: str | None,
        policy: dict[str, object],
        *,
        build_config: dict[str, object] | None = None,
    ) -> str:
        return view_profile(self.execution.snapshot, space, policy, build_config=build_config)

    async def current(self, collection: str) -> SnapshotDescriptor | None:
        """Return the current descriptor without decoding the candidate mapping."""
        async with self.execution.operation("wave.current"):
            return await self.snapshots.current(collection)

    async def describe(self, ref: str | SnapshotRef) -> SnapshotDescriptor:
        """Read frozen ownership and identity without loading candidates or Native."""
        async with self.execution.operation("wave.describe"):
            return await self.snapshots.describe(ref)

    async def describe_candidates(
        self, ref: str | SnapshotRef, keys: Sequence[str]
    ) -> tuple[CandidateDescriptor, ...]:
        """Read candidates in request order, including repeated keys, in one mapping read.

        Raises:
            NousError: A snapshot identity is invalid or requested keys are absent.
        """
        from nous.common.errors import NousError

        async with self.execution.operation("wave.describe_candidates"):
            row = await self.snapshots.resolve(ref)
            by_key = {item["key"]: item for item in row["mapping"]["candidates"]}
            missing = list(dict.fromkeys(key for key in keys if key not in by_key))
            if missing:
                raise NousError(
                    "wave.candidate_not_found",
                    "Candidates do not exist in this snapshot",
                    details={"candidates": missing},
                )
            owner = TypeAdapter(SnapshotOwner).validate_python(row["owner"])
            return tuple(
                CandidateDescriptor(
                    ref=CandidateRef(snapshot=row["identifier"], key=key),
                    owner=owner,
                    group_key=by_key[key]["group_key"],
                    content=ContentSelection.model_validate(by_key[key]["content"]),
                    origin_contents=tuple(
                        ContentSelection.model_validate(part)
                        for part in by_key[key]["origin_contents"]
                    ),
                    metadata=by_key[key]["metadata"],
                )
                for key in keys
            )

    async def build(self, manifest_path: Path, space: str | None = None) -> dict[str, object]:
        """Import a manifest and build and publish its immutable snapshot.

        ``space`` selects a configured embedding space; None follows the build
        configuration. The returned snapshot reference identifies the published
        result for subsequent queries.

        Returns:
            Build details including the newly built snapshot reference.

        Raises:
            NousError: ``wave.space_conflict`` when the manifest and explicit
                space differ, or an import, build or publication error.
        """
        async with self.execution.operation("wave.build") as context:
            view = await self.snapshots.import_manifest(manifest_path)
            result = await self.snapshots.build(view, space, context.settings)
            snapshot = cast(dict[str, str], result["snapshot"])
            record_completed(snapshot["identifier"])
            return result

    async def build_view(
        self,
        view: CorpusView,
        space: str | None = None,
        *,
        publish_if: Callable[[WriteTx], Awaitable[bool]] | None = None,
        build_key: str | None = None,
        build_config: dict[str, object] | None = None,
    ) -> dict[str, object]:
        """Build a snapshot from a captured typed corpus view.

        ``publish_if``, when provided, runs in the publication write transaction
        and determines whether the built snapshot becomes current. It can check
        whether the captured source generation is still eligible for publication.

        Returns:
            Build details including the snapshot reference and publication state.

        Raises:
            NousError: ``wave.space_conflict`` when the view and explicit space
                differ, or a build or publication error.
        """
        async with self.execution.operation("wave.build_view") as context:
            result = await self.snapshots.build(
                view,
                space,
                context.settings,
                publish_if=publish_if,
                build_key=build_key,
                build_config=build_config,
            )
            record_completed(cast(dict[str, str], result["snapshot"])["identifier"])
            return result

    async def query(self, request: WaveQuery) -> dict[str, object]:
        """Query the request's resolved snapshot and materialize source-aware hits.

        Returns:
            Hits, used retrieval lanes and query details for the resolved snapshot.

        Raises:
            NousError: For an unknown or mismatched snapshot, an empty or
                incompatible query, or a Native execution failure.
        """
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
                    "snapshot": row["identifier"],
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
                return {"snapshot": row["identifier"], "segments": []}
            self.snapshots.engine()
            import nous_native as native

            result = await self.execution.blocking(
                native.segment,
                vectors,
                context.settings.values.wave.context.segment_threshold,
            )
            ranges = json.loads(result.ranges_json)
            return {
                "snapshot": row["identifier"],
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
                    "identifier": row["identifier"],
                },
                "collection": row["mapping"]["name"],
                "space": row["space_definition"],
                "identity": row["identity"],
                "covered_generation": row["covered_generation"],
                "profile": row["profile"],
                "metadata": row["mapping"]["metadata"],
                "owner": row["mapping"]["owner"],
                "capabilities": list(snapshot.capabilities),
                "statistics": json.loads(snapshot.statistics_json),
                "source_roots": len(row["mapping"]["roots"]),
                "created_at": row["created_at"],
            }
