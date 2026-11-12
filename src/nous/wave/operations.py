"""Complete Wave use cases over immutable snapshots and source-aware query planning."""

from __future__ import annotations

import copy
import json
from collections.abc import Sequence
from contextlib import AbstractAsyncContextManager
from time import perf_counter

from pydantic import TypeAdapter

from nous.application.contracts import CallContext, define_operation
from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.common.operations import record_completed
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, ExternalInput
from nous.content.results import ResultStore
from nous.media.preparation import MediaPreparer
from nous.models.embeddings import resolved_space
from nous.models.vector_contracts import VectorRef
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.domains import TimeService

from .contracts import (
    BuiltProduct,
    CandidateDescriptor,
    CandidateFilter,
    CandidatePool,
    CandidateRef,
    ContextFragment,
    CorpusView,
    GroupDescriptor,
    ResolvedBuildProfile,
    ReusableDocument,
    SnapshotDescriptor,
    SnapshotRef,
    ThinkingStage,
    WaveBuild,
    WaveBuildResult,
    WaveCleanup,
    WaveCleanupResult,
    WaveInspection,
    WaveNeighborhoodResult,
    WaveQuery,
    WaveRead,
    WaveReadResult,
    WaveSearchResult,
    WaveSegment,
    WaveSegmentResult,
    WaveSnapshotRequest,
)
from .delivery import WaveDelivery
from .query import WaveQueries, require_bound_query
from .serialization import encode_json
from .snapshots import WaveSnapshots, build_profile, view_profile

WAVE_BUILD = define_operation(
    "wave.build",
    request=WaveBuild,
    result=WaveBuildResult,
    description="Build and publish a collection snapshot",
)
WAVE_QUERY = define_operation(
    "wave.query",
    request=WaveQuery,
    result=WaveSearchResult,
    description="Search a snapshot and return source passages",
)
WAVE_READ = define_operation(
    "wave.read",
    request=WaveRead,
    result=WaveReadResult,
    description="Read a candidate and its source selections",
)
WAVE_INSPECT = define_operation(
    "wave.inspect",
    request=WaveSnapshotRequest,
    result=WaveInspection | WaveNeighborhoodResult,
    description="Inspect a snapshot or its concept neighborhood",
)
WAVE_SEGMENT = define_operation(
    "wave.segment",
    request=WaveSegment,
    result=WaveSegmentResult,
    description="Group context fragments into segments",
)
WAVE_CLEANUP = define_operation(
    "wave.cleanup",
    request=WaveCleanup,
    result=WaveCleanupResult,
    description="Remove unretained snapshots and index components",
)


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
        self.delivery = WaveDelivery(artifacts, instance_id)

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
            return tuple(
                CandidateDescriptor(
                    ref=CandidateRef(snapshot=descriptor.ref.identifier, key=key),
                    group_key=by_key[key]["group_key"],
                    content=ContentSelection.model_validate(by_key[key]["content"]),
                    retrieval_content=ContentSelection.model_validate(
                        by_key[key]["retrieval_content"]
                    ),
                    origin_contents=tuple(
                        ContentSelection.model_validate(part)
                        for part in by_key[key]["origin_contents"]
                    ),
                )
                for key in keys
            )

    async def describe_groups(
        self, ref: SnapshotRef, keys: Sequence[str]
    ) -> dict[str, GroupDescriptor]:
        await self.snapshots.describe(ref)
        return await self.snapshots.groups(ref, keys)

    async def handle_build(self, request: WaveBuild, call: CallContext) -> WaveBuildResult:
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
            view = await self.snapshots.import_manifest(request, call)
            if request.space and view.space and request.space != view.space:
                raise NousError(
                    "wave.space_conflict", "Manifest and command select different spaces"
                )
            profile = self.resolve_build_profile(request.space or view.space)
            async with self.build_product(view, profile) as prepared:
                publish_started = perf_counter()
                async with self.database.write() as tx:
                    descriptor = await self.publish_in(tx, prepared)
                    await self.promote_in(tx, descriptor.ref)
            get_logger(__name__).info(
                "wave.publish.completed",
                extra={
                    "snapshot": descriptor.ref.identifier,
                    "publish_ms": (perf_counter() - publish_started) * 1000,
                },
            )
            record_completed(descriptor.ref)
            await call.saved(descriptor.ref)
            return WaveBuildResult.model_validate(
                {
                    "snapshot": descriptor.ref,
                    "collection": descriptor.collection,
                    "space": descriptor.space_id,
                    "covered_generation": descriptor.captured_generation,
                    "profile": descriptor.profile_signature,
                    "metadata": descriptor.metadata,
                    "current": True,
                    "identity": descriptor.identity,
                    **prepared.summary,
                }
            )

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

    def build_product(
        self, view: CorpusView, profile: ResolvedBuildProfile
    ) -> AbstractAsyncContextManager[BuiltProduct]:
        return self.snapshots.build_product(view, profile)

    async def match_units_in(
        self, tx: ReadTx, stamps: Sequence[str]
    ) -> dict[str, tuple[ReusableDocument, GroupDescriptor]]:
        return await self.snapshots.match_units_in(tx, stamps)

    async def related_units_in(
        self, tx: ReadTx, keys: Sequence[str]
    ) -> dict[str, ReusableDocument]:
        return await self.snapshots.related_units_in(tx, keys)

    async def publish_in(self, tx: WriteTx, product: BuiltProduct) -> SnapshotDescriptor:
        return await self.snapshots.publish_in(tx, product)

    async def promote_in(self, tx: WriteTx, ref: SnapshotRef) -> None:
        await self.snapshots.promote_in(tx, ref)

    async def retain_in(
        self, tx: WriteTx, ref: SnapshotRef, *, owner_kind: str, owner_id: str
    ) -> None:
        await self.snapshots.retain_in(tx, ref, owner_kind=owner_kind, owner_id=owner_id)

    async def release_in(self, tx: WriteTx, *, owner_kind: str, owner_id: str) -> None:
        await self.snapshots.release_in(tx, owner_kind=owner_kind, owner_id=owner_id)

    async def cleanup(
        self, *, keep_recent: int = 0, retain: Sequence[SnapshotRef] = ()
    ) -> WaveCleanupResult:
        async with self.execution.operation("wave.cleanup"):
            return await self.snapshots.cleanup(keep_recent=keep_recent, retain=retain)

    async def handle_cleanup(self, request: WaveCleanup, call: CallContext) -> WaveCleanupResult:
        retain = tuple(
            [
                (await self.describe(value)).ref if isinstance(value, str) else value
                for value in request.retain
            ]
        )
        return await self.cleanup(keep_recent=request.keep_recent, retain=retain)

    async def handle_query(self, request: WaveQuery, call: CallContext) -> WaveSearchResult:
        values = request.model_dump(mode="python")
        adapters = {
            "vector": TypeAdapter(VectorRef),
            "candidate_pool": TypeAdapter(CandidatePool),
            "context": TypeAdapter(tuple[ContextFragment, ...]),
            "candidate_filter": TypeAdapter(CandidateFilter),
            "thinking_stages": TypeAdapter(tuple[ThinkingStage, ...]),
        }
        for field, adapter in adapters.items():
            value = getattr(request, field)
            if isinstance(value, ExternalInput):
                selected = await call.inputs.read_text(
                    value.key, value.selection, call.settings.values.content.text_read_max_bytes
                )
                values[field] = adapter.validate_json(selected.data)
        return await self.query(WaveQuery.model_validate(values))

    async def handle_read(self, request: WaveRead, call: CallContext) -> WaveReadResult:
        return await self.read(request.snapshot, request.candidate, sources=request.sources)

    async def handle_inspect(
        self, request: WaveSnapshotRequest, call: CallContext
    ) -> WaveInspection | WaveNeighborhoodResult:
        if request.concepts:
            return await self.neighborhood(request.snapshot, request.concepts)
        return await self.inspect(request.snapshot)

    async def handle_segment(self, request: WaveSegment, call: CallContext) -> WaveSegmentResult:
        fragments = request.fragments
        if isinstance(fragments, ExternalInput):
            selected = await call.inputs.read_text(
                fragments.key, fragments.selection, call.settings.values.content.text_read_max_bytes
            )
            fragments = TypeAdapter(tuple[ContextFragment, ...]).validate_json(selected.data)
        return await self.segment(request.snapshot, fragments)

    async def query(self, request: WaveQuery) -> WaveSearchResult:
        """Query the request's resolved snapshot and materialize source-aware hits.

        Returns:
            Hits, used retrieval lanes and query details for the resolved snapshot.

        Raises:
            NousError: For an unknown or mismatched snapshot, an empty or
                incompatible query, or a Native execution failure.
        """
        require_bound_query(request)
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
                output = await self.delivery.materialize(request, row, result, plan.summary)
                get_logger(__name__).info(
                    "wave.query.completed",
                    extra={
                        "snapshot": row["identifier"],
                        "mode": plan.mode,
                        "returned": len(output.groups or output.hits),
                        "lanes": output.used_lanes,
                    },
                )
                return copy.deepcopy(output)

    async def read(self, ref: str, candidate_key: str, *, sources: bool = False) -> WaveReadResult:
        async with self.execution.operation("wave.read"):
            async with self.snapshots.pin(ref) as row:
                return copy.deepcopy(await self.queries.read(row, candidate_key, sources=sources))

    async def neighborhood(self, ref: str, concepts: Sequence[str]) -> WaveNeighborhoodResult:
        async with self.execution.operation("wave.neighborhood"):
            async with self.snapshots.borrow(ref) as (row, snapshot, token):
                rows = self.queries.neighborhood_rows(row, concepts)
                result = await self.snapshots.compute(
                    lambda: snapshot.neighborhood(rows, cancel=token), token
                )
                return copy.deepcopy(self.queries.neighborhood(row, concepts, result))

    async def segment(self, ref: str, fragments: Sequence[ContextFragment]) -> WaveSegmentResult:
        async with self.execution.operation("wave.segment") as context:
            async with self.snapshots.pin(ref) as row:
                vectors, keys = await self.queries.segment_vectors(row, fragments)
                snapshot_ref = SnapshotRef(
                    instance_id=self.snapshots.instance_id,
                    id=row["id"],
                    identifier=row["identifier"],
                )
                if not keys:
                    return WaveSegmentResult(snapshot=snapshot_ref, segments=(), dimensions=None)
                self.snapshots.engine()
                import nous_native as native

                result = await self.execution.blocking(
                    native.segment,
                    vectors,
                    context.settings.values.wave.context.segment_threshold,
                )
                ranges = json.loads(result.ranges_json)
                return WaveSegmentResult.model_validate(
                    {
                        "snapshot": snapshot_ref,
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
                )

    async def inspect(self, ref: str) -> WaveInspection:
        async with self.execution.operation("wave.inspect"):
            async with self.snapshots.borrow(ref) as (row, snapshot, _):
                return WaveInspection.model_validate(
                    {
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
                )
