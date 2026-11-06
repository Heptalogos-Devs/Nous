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
from uuid import UUID, uuid4, uuid5

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter
from sqlalchemy.dialects.sqlite import insert
from sqlalchemy.engine import RowMapping

from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.common.identifier import new_identifier
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
from .contracts import (
    CollectionManifest,
    CorpusView,
    Document,
    DocumentInput,
    SnapshotDescriptor,
    SnapshotOwner,
    SnapshotRef,
)

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    import nous_native


def encode_json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _signature(value: object) -> str:
    return hashlib.sha256(encode_json(value).encode("utf-8")).hexdigest()


def build_profile(settings: ConfigSnapshot) -> dict[str, object]:
    wave = settings.values.wave
    return {
        "native": wave.native.model_dump(mode="json")["build"],
        "candidate_grouping": "document",
        "origin_selections": "ordered",
        "associations": wave.associations.model_dump(mode="json"),
    }


def view_profile(
    settings: ConfigSnapshot,
    space: str | None,
    policy: dict[str, object],
    *,
    build_config: dict[str, object] | None = None,
) -> str:
    definition = resolved_space(settings, space) if space else None
    return _signature(
        {
            "space": definition.model_dump(mode="json") if definition else None,
            "build": build_config if build_config is not None else build_profile(settings),
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
            settings = self.execution.snapshot.values.wave.native
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
                            artifact=artifact.ref.identifier, selector=source.selector
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
        build_key: str | None = None,
        build_config: dict[str, object] | None = None,
    ) -> dict[str, object]:
        """Build one immutable view; a caller can guard its current-pointer update."""
        await self.time.validate(document.times for document in view.documents)
        if space and view.space and space != view.space:
            raise NousError("wave.space_conflict", "Manifest and command select different spaces")
        space_id = space or view.space
        definition = resolved_space(settings, space_id) if space_id else None
        profile_config = build_config if build_config is not None else build_profile(settings)
        build_signature = (
            _signature(
                {
                    "view": view.model_dump(mode="json"),
                    "build": profile_config,
                    "space": definition.model_dump(mode="json") if definition else None,
                }
            )
            if build_key
            else None
        )
        object_id = str(uuid5(UUID(self.instance_id), build_key)) if build_key else str(uuid4())
        identity = {
            "snapshot_id": object_id,
            "space_signature": _signature(
                definition.model_dump(mode="json") if definition else None
            ),
            "build_config_signature": _signature(profile_config),
        }
        async with self._build_locks.setdefault(view.name, asyncio.Lock()):
            if build_key:
                async with self.database.read() as tx:
                    previous = (
                        (
                            await tx.connection.execute(
                                sa.select(
                                    schema.snapshots,
                                    schema.collections.c.name,
                                    schema.collections.c.current_snapshot_id,
                                )
                                .join(
                                    schema.collections,
                                    schema.snapshots.c.collection_id == schema.collections.c.id,
                                )
                                .where(schema.snapshots.c.build_key == build_key)
                            )
                        )
                        .mappings()
                        .one_or_none()
                    )
                if previous is not None:
                    if previous["build_signature"] != build_signature:
                        raise NousError(
                            "wave.build_key_conflict",
                            "Build key identifies a different captured view",
                        )
                    return {
                        "snapshot": {
                            "instance_id": self.instance_id,
                            "id": previous["id"],
                            "identifier": previous["identifier"],
                        },
                        "collection": previous["name"],
                        "space": previous["space_id"],
                        "covered_generation": previous["covered_generation"],
                        "profile": previous["profile"],
                        "metadata": previous["metadata"],
                        "current": previous["current_snapshot_id"] == previous["id"],
                        "identity": previous["identity"],
                        **previous["build_summary"],
                    }
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
            staging_name = f".staging-{object_id}"
            staging = root / staging_name
            final = root / object_id
            final_exists = await self.execution.blocking(final.exists)
            if final_exists and not build_key:
                raise NousError("wave.snapshot_exists", "Snapshot directory already exists")
            published = final_exists
            token = native.CancellationToken()
            try:
                native_started = perf_counter()
                if not final_exists:
                    if build_key and await self.execution.blocking(staging.exists):
                        await self.execution.blocking(
                            self._remove_owned, staging, root, staging_name
                        )
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
                    if not final_exists:
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
                    identifier = ""
                    for _ in range(32):
                        identifier = new_identifier("wav")
                        inserted = await tx.connection.execute(
                            insert(schema.snapshots)
                            .values(
                                id=object_id,
                                identifier=identifier,
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
                                index_location=f"wave/{object_id}",
                                mapping=frozen.mapping,
                                owner=frozen.mapping["owner"],
                                metadata=frozen.mapping["metadata"],
                                build_key=build_key,
                                build_signature=build_signature,
                                build_summary={
                                    "capabilities": capabilities,
                                    "statistics": statistics,
                                },
                                created_at=self.clock.utc_now().isoformat(),
                            )
                            .on_conflict_do_nothing(index_elements=[schema.snapshots.c.identifier])
                        )
                        if inserted.rowcount == 1:
                            break
                    else:
                        raise NousError(
                            "wave.identifier_exhausted", "Cannot allocate a Wave identifier"
                        )
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
                            [{"snapshot_id": object_id, "artifact_id": ref} for ref in input_refs],
                        )
                    if frozen.vector_mappings:
                        await tx.connection.execute(
                            schema.snapshot_vectors.insert(),
                            [{"snapshot_id": object_id, **row} for row in frozen.vector_mappings],
                        )
                    make_current = publish_if is None or await publish_if(tx)
                    if make_current:
                        await tx.connection.execute(
                            schema.collections.update()
                            .where(schema.collections.c.id == collection_id)
                            .values(current_snapshot_id=object_id)
                        )
                self._cached_snapshot = (object_id, reopened)
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
                        "snapshot": identifier,
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
                    "id": object_id,
                    "identifier": identifier,
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

    def _descriptor(self, row: RowMapping) -> SnapshotDescriptor:
        return SnapshotDescriptor(
            ref=SnapshotRef(
                instance_id=self.instance_id, id=row["id"], identifier=row["identifier"]
            ),
            owner=TypeAdapter(SnapshotOwner).validate_python(row["owner"]),
            collection=row["name"],
            captured_generation=row["covered_generation"],
            profile_signature=row["profile"],
            space_id=row["space_id"],
            space_signature=row["identity"]["space_signature"],
            identity=row["identity"],
            metadata=row["metadata"],
        )

    async def _lookup(
        self, ref: str | SnapshotRef, *, include_mapping: bool = False, current: bool = False
    ) -> RowMapping | None:
        exact = ref if isinstance(ref, SnapshotRef) else None
        if exact is not None:
            if exact.instance_id != self.instance_id:
                raise NousError("wave.instance_mismatch", "Snapshot belongs to another instance")
            ref = exact.id
        columns = (
            list(schema.snapshots.c)
            if include_mapping
            else [
                schema.snapshots.c.id,
                schema.snapshots.c.identifier,
                schema.snapshots.c.owner,
                schema.snapshots.c.metadata,
                schema.snapshots.c.covered_generation,
                schema.snapshots.c.profile,
                schema.snapshots.c.space_id,
                schema.snapshots.c.identity,
            ]
        )
        query = sa.select(*columns, schema.collections.c.name).join(
            schema.collections, schema.snapshots.c.collection_id == schema.collections.c.id
        )
        condition = (
            sa.and_(
                schema.collections.c.name == ref,
                schema.collections.c.current_snapshot_id == schema.snapshots.c.id,
            )
            if current
            else sa.or_(schema.snapshots.c.id == ref, schema.snapshots.c.identifier == ref)
        )
        async with self.database.read() as tx:
            row = (await tx.connection.execute(query.where(condition))).mappings().one_or_none()
            if row is None and exact is None and not current:
                row = (
                    (
                        await tx.connection.execute(
                            query.where(
                                schema.collections.c.name == ref,
                                schema.collections.c.current_snapshot_id == schema.snapshots.c.id,
                            )
                        )
                    )
                    .mappings()
                    .one_or_none()
                )
        if (
            row is not None
            and exact is not None
            and (row["id"] != exact.id or row["identifier"] != exact.identifier)
        ):
            raise NousError(
                "wave.snapshot_mismatch", "Snapshot reference differs from its identity"
            )
        return row

    async def current(self, collection: str) -> SnapshotDescriptor | None:
        row = await self._lookup(collection, current=True)
        return self._descriptor(row) if row is not None else None

    async def describe(self, ref: str | SnapshotRef) -> SnapshotDescriptor:
        """Read only immutable identity, ownership and summary columns."""
        row = await self._lookup(ref)
        if row is None:
            raise NousError(
                "wave.snapshot_not_found", "Wave snapshot does not exist", details={"ref": str(ref)}
            )
        return self._descriptor(row)

    async def resolve(self, ref: str | SnapshotRef) -> SnapshotRow:
        row = await self._lookup(ref, include_mapping=True)
        if row is None:
            raise NousError(
                "wave.snapshot_not_found", "Wave snapshot does not exist", details={"ref": str(ref)}
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
