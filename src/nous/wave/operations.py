"""Complete Wave use cases over immutable snapshots and source-aware query planning."""

from __future__ import annotations

import json
from collections.abc import Sequence
from pathlib import Path
from time import perf_counter
from typing import cast

import numpy as np

from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.common.operations import record_completed
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection
from nous.content.results import ResultStore
from nous.media.preparation import MediaPreparer
from nous.models.embeddings import resolved_space
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database, WriteTx
from nous.temporal.domains import TimeService

from .contracts import (
    CandidateDescriptor,
    CandidateRef,
    ContextFragment,
    CorpusView,
    PreparedSnapshot,
    ResolvedBuildProfile,
    SnapshotDescriptor,
    SnapshotRef,
    WaveQuery,
)
from .query import WaveQueries
from .snapshots import WaveSnapshots, build_profile, encode_json, view_profile


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
        self.execution, self.database = execution, database
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
        async with self.execution.operation("wave.describe_candidates"):
            descriptor = await self.snapshots.describe(ref)
            by_key = await self.snapshots.candidates(descriptor.ref, keys)
            missing = list(dict.fromkeys(key for key in keys if key not in by_key))
            if missing:
                raise NousError(
                    "wave.candidate_not_found",
                    "Candidates do not exist in this snapshot",
                    details={"candidates": missing},
                )
            owner = descriptor.owner
            return tuple(
                CandidateDescriptor(
                    ref=CandidateRef(snapshot=descriptor.ref.identifier, key=key),
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
        async with self.execution.operation("wave.build"):
            view = await self.snapshots.import_manifest(manifest_path)
            if space and view.space and space != view.space:
                raise NousError(
                    "wave.space_conflict", "Manifest and command select different spaces"
                )
            profile = self.resolve_build_profile(space or view.space)
            prepared = await self.prepare_build(view, profile)
            publish_started = perf_counter()
            async with self.database.write() as tx:
                descriptor = await self.accept_in(tx, prepared)
                await self.promote_in(tx, descriptor.ref)
            self.release_prepared(prepared)
            get_logger(__name__).info(
                "wave.publish.completed",
                extra={
                    "snapshot": descriptor.ref.identifier,
                    "publish_ms": (perf_counter() - publish_started) * 1000,
                },
            )
            record_completed(descriptor.ref.identifier)
            return {
                "snapshot": descriptor.ref.model_dump(mode="json"),
                "collection": descriptor.collection,
                "space": descriptor.space_id,
                "covered_generation": descriptor.captured_generation,
                "profile": descriptor.profile_signature,
                "metadata": descriptor.metadata,
                "current": True,
                "identity": descriptor.identity,
                **prepared.summary,
            }

    def resolve_build_profile(
        self,
        space: str | None,
        *,
        build_config: dict[str, object] | None = None,
    ) -> ResolvedBuildProfile:
        settings = self.execution.snapshot
        return ResolvedBuildProfile.model_validate(
            {
                "space_id": space,
                "space_definition": resolved_space(settings, space) if space else None,
                "build_config": build_config
                if build_config is not None
                else build_profile(settings),
            }
        )

    async def prepare_build(
        self, view: CorpusView, profile: ResolvedBuildProfile, *, build_key: str | None = None
    ) -> PreparedSnapshot:
        async with self.execution.operation("wave.prepare_build"):
            return await self.snapshots.prepare_build(view, profile, build_key=build_key)

    async def accept_in(self, tx: WriteTx, prepared: PreparedSnapshot) -> SnapshotDescriptor:
        return await self.snapshots.accept_in(tx, prepared)

    async def promote_in(self, tx: WriteTx, ref: SnapshotRef) -> None:
        await self.snapshots.promote_in(tx, ref)

    def release_prepared(self, prepared: PreparedSnapshot) -> None:
        self.snapshots.release_prepared(prepared)

    async def retain_in(
        self, tx: WriteTx, ref: SnapshotRef, *, owner_kind: str, owner_id: str
    ) -> None:
        await self.snapshots.retain_in(tx, ref, owner_kind=owner_kind, owner_id=owner_id)

    async def release_in(self, tx: WriteTx, *, owner_kind: str, owner_id: str) -> None:
        await self.snapshots.release_in(tx, owner_kind=owner_kind, owner_id=owner_id)

    async def cleanup(
        self, *, keep_recent: int = 0, retain: Sequence[SnapshotRef] = ()
    ) -> dict[str, object]:
        async with self.execution.operation("wave.cleanup"):
            return await self.snapshots.cleanup(keep_recent=keep_recent, retain=retain)

    async def query(self, request: WaveQuery) -> dict[str, object]:
        """Query the request's resolved snapshot and materialize source-aware hits.

        Returns:
            Hits, used retrieval lanes and query details for the resolved snapshot.

        Raises:
            NousError: For an unknown or mismatched snapshot, an empty or
                incompatible query, or a Native execution failure.
        """
        async with self.execution.operation("wave.query"):
            async with self.snapshots.borrow(request.snapshot) as (row, snapshot, token):
                plan = await self.queries.prepare(request, row)
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
                local_passages: dict[int, tuple[int, dict[str, object]]] = {}
                if plan.mode != "lexical":
                    primary = cast(dict[str, object], json.loads(result.result_json))
                    candidates = row["mapping"]["candidates"]
                    groups: dict[int, str] = {}
                    for hit in cast(list[dict[str, object]], primary["hits"]):
                        index = cast(int, hit["row"])
                        candidate = candidates[index]
                        evidence = cast(dict[str, object], hit.get("lexical_evidence") or {})
                        selection = ContentSelection.model_validate(candidate["content"])
                        if (
                            selection.selection.is_whole
                            and evidence.get("body_range") is None
                            and "title" not in cast(list[str], evidence.get("matched_fields", []))
                        ):
                            groups[index] = candidate["group_key"]
                    allowed = set(int(value) for value in plan.allowed)
                    targets = set(groups.values())
                    local_rows = [
                        index
                        for index, candidate in enumerate(candidates)
                        if index in allowed
                        and candidate["group_key"] in targets
                        and not ContentSelection.model_validate(
                            candidate["content"]
                        ).selection.is_whole
                    ]
                    if local_rows:
                        options: dict[str, object] = {
                            **plan.options,
                            "top_k": len(targets),
                            "grouping": "document",
                            "passages_per_group": 1,
                            "include_trace": False,
                            "include_counterevidence": False,
                            "thinking_stages": [],
                        }
                        local_result = await self.snapshots.compute(
                            lambda: snapshot.search(
                                mode=plan.mode,
                                text=plan.text,
                                vector=plan.vector,
                                ghost_vectors=plan.ghosts,
                                allowed_rows=np.asarray(local_rows, dtype=np.uint64),
                                options_json=encode_json(options),
                                candidate_pool=plan.pool,
                                cancel=token,
                            ),
                            token,
                        )
                        local = cast(dict[str, object], json.loads(local_result.result_json))
                        chosen: dict[str, tuple[int, dict[str, object]]] = {}
                        for hit in cast(list[dict[str, object]], local["hits"]):
                            index = cast(int, hit["row"])
                            chosen.setdefault(
                                candidates[index]["group_key"],
                                (index, cast(dict[str, object], hit.get("lexical_evidence") or {})),
                            )
                        local_passages = {
                            index: chosen[group]
                            for index, group in groups.items()
                            if group in chosen
                            and ContentSelection.model_validate(
                                candidates[index]["content"]
                            ).artifact
                            == ContentSelection.model_validate(
                                candidates[chosen[group][0]]["content"]
                            ).artifact
                        }
                output = await self.queries.materialize(
                    request, row, result, plan.summary, local_passages=local_passages
                )
                get_logger(__name__).info(
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
            async with self.snapshots.pin(ref) as row:
                return await self.queries.read(row, candidate_key, sources=sources)

    async def neighborhood(self, ref: str, concepts: Sequence[str]) -> dict[str, object]:
        async with self.execution.operation("wave.neighborhood"):
            async with self.snapshots.borrow(ref) as (row, snapshot, token):
                rows = self.queries.neighborhood_rows(row, concepts)
                result = await self.snapshots.compute(
                    lambda: snapshot.neighborhood(rows, cancel=token), token
                )
                return self.queries.neighborhood(row, concepts, result)

    async def segment(self, ref: str, fragments: Sequence[ContextFragment]) -> dict[str, object]:
        async with self.execution.operation("wave.segment") as context:
            async with self.snapshots.pin(ref) as row:
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
            async with self.snapshots.borrow(ref) as (row, snapshot, _):
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
