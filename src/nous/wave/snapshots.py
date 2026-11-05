"""Immutable snapshot construction, publication, identity and Native resource lifetime."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import shutil
from collections.abc import Awaitable, Callable
from pathlib import Path
from time import perf_counter
from typing import TYPE_CHECKING, cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue
from sqlalchemy.dialects.sqlite import insert

from nous.application.embedding import EmbeddingOperations
from nous.common.address import new_address
from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ArtifactRef, ContentSelection, InputSpec
from nous.content.results import ResultStore
from nous.media.preparation import MediaPreparer
from nous.models.embeddings import resolved_space
from nous.runtime.execution import ExecutionManager
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database, WriteTx
from nous.temporal.domains import TimeService

from . import schema
from .assets import FrozenInputs, SnapshotRow, content_evidence, freeze_inputs
from .configuration import require_native_profiles
from .contracts import CollectionManifest, CorpusView, Document, DocumentInput, SnapshotRef

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    import nous_native


def encode_json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _signature(value: object) -> str:
    return hashlib.sha256(encode_json(value).encode("utf-8")).hexdigest()


def build_profile(settings: ConfigSnapshot) -> dict[str, object]:
    wave = settings.typed_values().wave
    return {
        "native": wave.native.build,
        "candidate_grouping": "document",
        "origin_selections": "ordered",
        "associations": wave.associations.model_dump(mode="json"),
    }


def view_profile(settings: ConfigSnapshot, space: str | None, policy: dict[str, object]) -> str:
    definition = resolved_space(settings, space) if space else None
    return _signature(
        {
            "space": definition.model_dump(mode="json") if definition else None,
            "build": build_profile(settings),
            "policy": policy,
        }
    )


class WaveSnapshots:
    """Own SQL references, immutable directories and the loaded Native engine/snapshot."""

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
        self.database, self.execution, self.paths = database, execution, paths
        self.instance_id, self.time, self.clock = instance_id, time, time.clock
        self.artifacts, self.results, self.embedding, self.preparer = (
            artifacts,
            results,
            embedding,
            preparer,
        )
        self._engine: nous_native.NativeEngine | None = None
        self._build_locks: dict[str, asyncio.Lock] = {}
        self._cached_snapshot: tuple[str, nous_native.NativeSnapshot] | None = None
        self._open_lock = asyncio.Lock()

    def engine(self) -> nous_native.NativeEngine:
        try:
            import nous_native as native
        except ImportError as error:
            raise NousError(
                "wave.native_unavailable",
                "Wave Native is not installed; build the native extension for this environment.",
                cause=error,
            ) from error
        require_native_profiles()
        if self._engine is None:
            settings = self.execution.snapshot.typed_values().wave.native
            self._engine = native.NativeEngine(encode_json({"threads": settings.threads}))
        return self._engine

    def close(self) -> None:
        if self._engine is not None:
            self._engine.close()
            self._engine = None
        self._cached_snapshot = None

    async def status(self) -> dict[str, object]:
        """Read registered scale and already-loaded capabilities without opening an engine."""
        async with self.database.read() as tx:
            row = (
                (
                    await tx.connection.execute(
                        sa.select(
                            sa.func.count().label("snapshots"),
                            sa.func.coalesce(
                                sa.func.sum(
                                    sa.func.json_array_length(
                                        schema.snapshots.c.mapping, "$.candidates"
                                    )
                                ),
                                0,
                            ).label("candidates"),
                            sa.func.coalesce(
                                sa.func.sum(
                                    sa.func.json_array_length(
                                        schema.snapshots.c.mapping, "$.concepts"
                                    )
                                ),
                                0,
                            ).label("concepts"),
                        )
                    )
                )
                .mappings()
                .one()
            )
            collections = await tx.connection.scalar(
                sa.select(sa.func.count()).select_from(schema.collections)
            )
        cached = self._cached_snapshot
        return {
            "collections": collections or 0,
            "snapshots": row["snapshots"],
            "candidates": row["candidates"],
            "concepts": row["concepts"],
            "native_loaded": self._engine is not None,
            "loaded_snapshot": cached[0] if cached else None,
            "loaded_channels": list(cached[1].capabilities) if cached else [],
        }

    async def compute[T](self, call: Callable[[], T], token: nous_native.CancellationToken) -> T:
        from nous_native import NativeError

        try:
            async with self.execution.operation("wave.native"):
                return await self.execution.blocking(call, on_cancel=token.cancel)
        except NativeError as error:
            try:
                details = cast(dict[str, JsonValue], json.loads(error.details_json))
            except TypeError, ValueError:
                details = {}
            raise NousError(error.code, str(error), details=details, cause=error) from error

    @staticmethod
    def _remove_owned(directory: Path, root: Path, expected_name: str) -> None:
        resolved = directory.resolve()
        if resolved.parent != root.resolve() or resolved.name != expected_name:
            raise NousError("wave.invalid_path", "Refusing to remove an unrelated index directory")
        if resolved.exists():
            shutil.rmtree(resolved)

    @staticmethod
    def _bundles(
        frozen: FrozenInputs,
    ) -> tuple[
        nous_native.Candidates,
        nous_native.Concepts,
        nous_native.SourceFacts,
        nous_native.Associations,
    ]:
        import nous_native as native

        return (
            native.Candidates(
                encode_json(frozen.candidate_metadata),
                frozen.candidate_texts,
                frozen.candidate_vectors,
                frozen.membership_indptr,
                frozen.membership_indices,
                frozen.path_indptr,
                frozen.path_step_indptr,
                frozen.path_indices,
                frozen.path_positions,
                frozen.candidate_root_indptr,
                frozen.candidate_root_indices,
            ),
            native.Concepts(encode_json(frozen.concept_metadata), frozen.concept_vectors),
            native.SourceFacts(
                encode_json(frozen.facts_metadata),
                frozen.sequence_roots,
                frozen.sequence_indptr,
                frozen.sequence_indices,
                frozen.sequence_positions,
                frozen.sequence_ordered,
            ),
            native.Associations(
                encode_json(frozen.association_metadata),
                frozen.association_sources,
                frozen.association_targets,
                frozen.association_directed,
                frozen.association_strengths,
                frozen.association_confidences,
                frozen.association_polarities,
                frozen.association_edge_groups,
                frozen.group_root_indptr,
                frozen.group_root_indices,
            ),
        )

    async def import_manifest(self, manifest_path: Path) -> CorpusView:
        """Accept a file-based collection and freeze its document identities."""
        path = await self.execution.blocking(manifest_path.resolve)
        manifest_artifact = await self.artifacts.ingest(path)
        manifest = CollectionManifest.model_validate_json(
            await self.artifacts.read_text(manifest_artifact.ref)
        )
        documents: list[Document] = []
        accepted: dict[str, ArtifactRef] = {}
        for document in manifest.documents:
            source = document.input
            if source.file is not None:
                file = Path(source.file)
                file = file if file.is_absolute() else path.parent / file
                spec = InputSpec(kind="file", path=str(file.resolve()))
            elif source.text is not None:
                spec = InputSpec(kind="text", text=source.text, display_name=document.key)
            else:
                assert source.artifact is not None
                spec = InputSpec(kind="artifact", artifact=source.artifact)
            artifact = await self.artifacts.accept(spec)
            accepted[document.key] = artifact.ref
            documents.append(
                document.model_copy(
                    update={
                        "input": DocumentInput(
                            artifact=artifact.ref.address, selector=source.selector
                        )
                    }
                )
            )
        return CorpusView(
            **manifest.model_dump(exclude={"documents"}),
            documents=tuple(documents),
            manifest_artifact=manifest_artifact.ref,
            evidence=await content_evidence(documents, accepted, self.results),
        )

    async def build(
        self,
        view: CorpusView,
        space: str | None,
        settings: ConfigSnapshot,
        *,
        publish_if: Callable[[WriteTx], Awaitable[bool]] | None = None,
    ) -> dict[str, object]:
        """Build one immutable view; a caller can guard its current-pointer update."""
        await self.time.validate(document.times for document in view.documents)
        if space and view.space and space != view.space:
            raise NousError("wave.space_conflict", "Manifest and command select different spaces")
        space_id = space or view.space
        definition = resolved_space(settings, space_id) if space_id else None
        profile_config = build_profile(settings)
        identifier = str(uuid4())
        identity = {
            "snapshot_id": identifier,
            "space_signature": _signature(
                definition.model_dump(mode="json") if definition else None
            ),
            "build_config_signature": _signature(profile_config),
        }
        async with self._build_locks.setdefault(view.name, asyncio.Lock()):
            frozen = await freeze_inputs(
                view,
                space_id,
                self.artifacts,
                self.embedding,
                self.preparer,
            )
            engine = self.engine()
            import nous_native as native

            bundles = self._bundles(frozen)
            root = self.paths.resolve_owned("index", "wave")
            await self.execution.blocking(root.mkdir, parents=True, exist_ok=True)
            staging_name = f".staging-{identifier}"
            staging = root / staging_name
            final = root / identifier
            if await self.execution.blocking(final.exists):
                raise NousError("wave.snapshot_exists", "Snapshot directory already exists")
            published = False
            token = native.CancellationToken()
            try:
                native_started = perf_counter()
                await self.compute(
                    lambda: engine.build(
                        *bundles,
                        encode_json(identity),
                        str(staging),
                        options_json=encode_json(profile_config),
                        cancel=token,
                    ),
                    token,
                )
                native_ms = (perf_counter() - native_started) * 1000
                publish_started = perf_counter()
                try:
                    await self.execution.blocking(os.replace, staging, final)
                except asyncio.CancelledError:
                    # The worker may have completed the rename before cancellation surfaced.
                    published = final.is_dir() and not staging.exists()
                    raise
                else:
                    published = True
                # Reopen exactly what will be referenced before making it current.
                reopened = await self.compute(
                    lambda: engine.open(str(final), encode_json(identity), cancel=token), token
                )
                capabilities = list(reopened.capabilities)
                statistics = json.loads(reopened.statistics_json)
                async with self.database.write() as tx:
                    await tx.connection.execute(
                        insert(schema.collections)
                        .values(id=str(uuid4()), name=view.name)
                        .on_conflict_do_nothing(index_elements=[schema.collections.c.name])
                    )
                    collection_id = await tx.connection.scalar(
                        sa.select(schema.collections.c.id).where(
                            schema.collections.c.name == view.name
                        )
                    )
                    if collection_id is None:
                        raise NousError("wave.collection_unavailable", "Cannot create collection")
                    address = ""
                    for _ in range(32):
                        address = new_address("wav")
                        inserted = await tx.connection.execute(
                            insert(schema.snapshots)
                            .values(
                                id=identifier,
                                address=address,
                                collection_id=collection_id,
                                manifest_artifact_id=(
                                    view.manifest_artifact.id if view.manifest_artifact else None
                                ),
                                covered_generation=view.generation,
                                profile=view.profile,
                                space_id=space_id,
                                space_definition=definition.model_dump(mode="json")
                                if definition
                                else None,
                                identity=identity,
                                index_location=f"wave/{identifier}",
                                mapping=frozen.mapping,
                                created_at=self.clock.utc_now().isoformat(),
                            )
                            .on_conflict_do_nothing(index_elements=[schema.snapshots.c.address])
                        )
                        if inserted.rowcount == 1:
                            break
                    else:
                        raise NousError("wave.address_exhausted", "Cannot allocate a Wave address")
                    documents = cast(dict[str, dict[str, object]], frozen.mapping["documents"])
                    roots = frozen.mapping["roots"]
                    input_refs: set[str] = (
                        {view.manifest_artifact.id} if view.manifest_artifact else set()
                    )
                    for document in documents.values():
                        input_refs.add(cast(dict[str, str], document["artifact"])["id"])
                    for refs in roots.values():
                        input_refs.update(ref["id"] for ref in refs)
                    for candidate in frozen.mapping["candidates"]:
                        if candidate["annotation"] is not None:
                            input_refs.add(candidate["annotation"]["id"])
                        input_refs.add(
                            ContentSelection.model_validate(candidate["content"]).artifact.id
                        )
                        input_refs.add(
                            ContentSelection.model_validate(
                                candidate["retrieval_content"]
                            ).artifact.id
                        )
                    for row in frozen.vector_mappings:
                        input_refs.add(
                            cast(dict[str, dict[str, str]], row["input"])["artifact"]["id"]
                        )
                    if input_refs:
                        await tx.connection.execute(
                            schema.snapshot_artifacts.insert(),
                            [{"snapshot_id": identifier, "artifact_id": ref} for ref in input_refs],
                        )
                    if frozen.vector_mappings:
                        await tx.connection.execute(
                            schema.snapshot_vectors.insert(),
                            [{"snapshot_id": identifier, **row} for row in frozen.vector_mappings],
                        )
                    make_current = publish_if is None or await publish_if(tx)
                    if make_current:
                        await tx.connection.execute(
                            schema.collections.update()
                            .where(schema.collections.c.id == collection_id)
                            .values(current_snapshot_id=identifier)
                        )
                self._cached_snapshot = (identifier, reopened)
                statistics["preparation"] = frozen.preparation_stats
                statistics["stages_ms"] = {
                    "content_prepare": frozen.preparation_stats["content_prepare_ms"],
                    "embedding": frozen.preparation_stats["embedding_ms"],
                    "native_build": native_ms,
                    "publish": (perf_counter() - publish_started) * 1000,
                }
                logger.info(
                    "wave.build.completed",
                    extra={
                        "snapshot": address,
                        "preparation": frozen.preparation_stats,
                        "stages_ms": statistics["stages_ms"],
                    },
                )
            except BaseException:
                if not published:
                    await self.execution.blocking(self._remove_owned, staging, root, staging_name)
                raise
            return {
                "snapshot": {
                    "instance_id": self.instance_id,
                    "id": identifier,
                    "address": address,
                },
                "collection": view.name,
                "space": space_id,
                "covered_generation": view.generation,
                "profile": view.profile,
                "metadata": view.metadata,
                "current": make_current,
                "capabilities": capabilities,
                "statistics": statistics,
                "identity": identity,
            }

    async def current(self, collection: str) -> SnapshotRow | None:
        async with self.database.read() as tx:
            row = (
                (
                    await tx.connection.execute(
                        sa.select(schema.snapshots)
                        .join(
                            schema.collections,
                            schema.collections.c.current_snapshot_id == schema.snapshots.c.id,
                        )
                        .where(schema.collections.c.name == collection)
                    )
                )
                .mappings()
                .one_or_none()
            )
        return cast(SnapshotRow, dict(row)) if row is not None else None

    async def resolve(self, ref: str | SnapshotRef) -> SnapshotRow:
        exact = ref if isinstance(ref, SnapshotRef) else None
        if exact is not None:
            if exact.instance_id != self.instance_id:
                raise NousError("wave.instance_mismatch", "Snapshot belongs to another instance")
            ref = exact.id
        async with self.database.read() as tx:
            row = (
                (
                    await tx.connection.execute(
                        sa.select(schema.snapshots).where(
                            sa.or_(schema.snapshots.c.id == ref, schema.snapshots.c.address == ref)
                        )
                    )
                )
                .mappings()
                .one_or_none()
            )
            if row is None and exact is None:
                row = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.snapshots)
                            .join(
                                schema.collections,
                                schema.collections.c.current_snapshot_id == schema.snapshots.c.id,
                            )
                            .where(schema.collections.c.name == ref)
                        )
                    )
                    .mappings()
                    .one_or_none()
                )
        if row is None:
            raise NousError(
                "wave.snapshot_not_found", "Wave snapshot does not exist", details={"ref": ref}
            )
        if exact is not None and (row["id"] != exact.id or row["address"] != exact.address):
            raise NousError(
                "wave.snapshot_mismatch", "Snapshot reference differs from its identity"
            )
        return cast(SnapshotRow, dict(row))

    async def open(
        self, row: SnapshotRow
    ) -> tuple[nous_native.NativeSnapshot, nous_native.CancellationToken]:
        engine = self.engine()
        import nous_native as native

        token = native.CancellationToken()
        async with self._open_lock:
            if self._cached_snapshot is not None and self._cached_snapshot[0] == row["id"]:
                snapshot = self._cached_snapshot[1]
            else:
                path = self.paths.resolve_owned("index", row["index_location"])
                snapshot = await self.compute(
                    lambda: engine.open(str(path), encode_json(row["identity"]), cancel=token),
                    token,
                )
                self._cached_snapshot = (row["id"], snapshot)
        return snapshot, token
