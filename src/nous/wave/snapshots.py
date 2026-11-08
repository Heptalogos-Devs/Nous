"""Immutable snapshot construction, publication, identity and Native resource lifetime."""

from __future__ import annotations

import asyncio
import json
import shutil
from collections.abc import AsyncGenerator, Callable, Sequence
from contextlib import AsyncExitStack, asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from time import perf_counter
from typing import TYPE_CHECKING, cast
from uuid import UUID, uuid5

import sqlalchemy as sa
from pydantic import JsonValue

from nous.application.embedding import EmbeddingOperations
from nous.application.preparation import TextPreparation
from nous.common.errors import NousError
from nous.common.identifier import new_identifier
from nous.common.keyed_locks import KeyedLocks
from nous.configuration import ConfigSnapshot
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection
from nous.content.results import ResultStore
from nous.media.preparation import MediaPreparer
from nous.models.embeddings import resolved_space
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager, complete_cleanup
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database
from nous.temporal.domains import TimeService

from . import schema
from .assets import freeze_inputs
from .configuration import require_native_profiles
from .contracts import (
    CorpusView,
    PreparedSnapshot,
    ProjectionUnit,
    ResolvedBuildProfile,
    SnapshotDraft,
    SnapshotRef,
)
from .importer import ManifestImporter
from .mapping import CandidateMapping, SnapshotRow
from .native_inputs import bundles
from .native_inputs import component_signature as native_component_signature
from .projection import projection_inputs
from .reading import SnapshotReader
from .registry import SnapshotRegistry
from .serialization import content_signature, encode_json

if TYPE_CHECKING:
    import nous_native


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
    return content_signature(
        {
            "space": definition.model_dump(mode="json") if definition else None,
            "build": build_config if build_config is not None else build_profile(settings),
            "policy": policy,
        }
    )


@dataclass
class _Preparation:
    users: int = 0
    ready: int = 0
    asset: str | None = None


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
        self.registry = SnapshotRegistry(instance_id)
        self.reader = SnapshotReader(database, instance_id, self.registry)
        self.current = self.reader.current
        self.describe = self.reader.describe
        self.candidates = self.reader.candidates
        self.resolve = self.reader.resolve
        self.accept_in = self.registry.accept_in
        self.promote_in = self.registry.promote_in
        self.retain_in = self.registry.retain_in
        self.release_in = self.registry.release_in
        self.artifacts, self.results, self.embedding, self.preparer = (
            artifacts,
            results,
            embedding,
            preparer,
        )
        self.importer = ManifestImporter(artifacts, results, execution)
        self.import_manifest = self.importer.import_manifest
        self._engine: nous_native.NativeEngine | None = None
        self._build_locks = KeyedLocks[str]()
        self._component_locks = KeyedLocks[str]()
        self._cached_snapshot: tuple[str, nous_native.NativeSnapshot] | None = None
        self._cached_mapping: tuple[str, SnapshotRow] | None = None
        self._mapping_lock = asyncio.Lock()
        self._open_lock = asyncio.Lock()
        self._lifetime_lock = asyncio.Lock()
        self._borrowed: dict[str, int] = {}
        self._prepared: dict[str, _Preparation] = {}

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
        self._cached_mapping = None

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
                                    sa.func.json_array_length(schema.projection_graphs.c.concepts)
                                ),
                                0,
                            ).label("concepts"),
                        )
                        .select_from(schema.snapshots.join(schema.projection_graphs))
                        .where(schema.snapshots.c.accepted.is_(True))
                    )
                )
                .mappings()
                .one()
            )
            collections = await tx.connection.scalar(
                sa.select(sa.func.count(sa.distinct(schema.snapshots.c.collection_id))).where(
                    schema.snapshots.c.accepted.is_(True)
                )
            )
            candidates = await tx.connection.scalar(
                sa.select(sa.func.count())
                .select_from(schema.snapshot_candidates.join(schema.snapshots))
                .where(schema.snapshots.c.accepted.is_(True))
            )
        cached = self._cached_snapshot
        return {
            "collections": collections or 0,
            "snapshots": row["snapshots"],
            "candidates": candidates or 0,
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
    def _entries(directory: Path) -> list[Path]:
        return list(directory.iterdir())

    @staticmethod
    def _component_state(path: Path) -> str:
        """Distinguish an absent or interrupted build from a published component."""
        try:
            path.stat()
        except FileNotFoundError:
            return "missing"
        if not path.is_dir():
            raise NousError("wave.invalid_path", "Component path is not a directory")
        try:
            (path / "manifest.json").stat()
        except FileNotFoundError:
            return "incomplete"
        return "complete"

    async def _remove_incomplete_component(self, path: Path, location: str) -> None:
        """Discard an interrupted component only when no durable result owns it."""
        async with self._lifetime_lock:
            async with self.database.read() as tx:
                referenced = await tx.connection.scalar(
                    sa.select(sa.exists().where(schema.snapshots.c.index_location == location))
                )
            if referenced or any(
                state.ready > 0 and state.asset == location for state in self._prepared.values()
            ):
                raise NousError(
                    "wave.snapshot_invalid",
                    "An owned Native component has no complete manifest",
                    details={"location": location},
                )
            await self.execution.blocking(self._remove_owned, path, path.parent, path.name)

    @asynccontextmanager
    async def prepare_build(
        self,
        view: CorpusView,
        profile: ResolvedBuildProfile,
        *,
        build_key: str | None = None,
    ) -> AsyncGenerator[PreparedSnapshot]:
        # Pydantic's frozen models can still contain caller-owned mutable dictionaries.
        view, profile = view.model_copy(deep=True), profile.model_copy(deep=True)
        signature = content_signature(
            {"view": view.model_dump(mode="json"), "profile": profile.model_dump(mode="json")}
        )
        key = build_key or f"view:{signature}"
        object_id = str(uuid5(UUID(self.instance_id), key))
        state = self._prepared.setdefault(object_id, _Preparation())
        state.users += 1
        try:
            async with self.execution.operation("wave.prepare_build"):
                prepared = await self._prepare_build(
                    view, profile, key, object_id, signature, state
                )
            state.ready += 1
            try:
                yield prepared
            finally:
                state.ready -= 1
        finally:
            state.users -= 1
            if state.users == 0:
                del self._prepared[object_id]

    async def _prepare_build(
        self,
        view: CorpusView,
        profile: ResolvedBuildProfile,
        key: str,
        object_id: str,
        build_signature: str,
        state: _Preparation,
    ) -> PreparedSnapshot:
        """Construct immutable files and a recoverable manifest outside a transaction."""
        await self.time.validate(document.times for document in view.documents)
        if view.space is not None and view.space != profile.space_id:
            raise NousError("wave.space_conflict", "Manifest and build select different spaces")
        root = self.paths.resolve_owned("index", "wave")
        component_root = root / "components"
        await self.execution.blocking(component_root.mkdir, parents=True, exist_ok=True)
        async with self._build_locks.hold(key):
            async with self.database.read() as tx:
                existing = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.snapshots).where(schema.snapshots.c.id == object_id)
                        )
                    )
                    .mappings()
                    .one_or_none()
                )
            if existing is not None:
                if existing["build_signature"] != build_signature:
                    raise NousError(
                        "wave.build_key_conflict", "Build key identifies a different captured view"
                    )
                import nous_native as native

                token = native.CancellationToken()
                component = self.paths.resolve_owned("index", existing["index_location"])
                async with self._component_locks.hold(existing["component_signature"]):
                    await self.compute(
                        lambda: self.engine().open(
                            str(component), encode_json(existing["identity"]), cancel=token
                        ),
                        token,
                    )
                return PreparedSnapshot(
                    ref=SnapshotRef(
                        instance_id=self.instance_id,
                        id=object_id,
                        identifier=existing["identifier"],
                    ),
                    build_signature=build_signature,
                    summary=existing["build_summary"],
                )
            inputs = projection_inputs(view)
            documents = {item.key: item for item in view.documents}
            input_documents = {
                name: [
                    cast(str, item["key"])
                    for item in cast(list[dict[str, object]], value["documents"])
                ]
                for name, value in inputs.items()
                if value["declarations"]
            }
            requested = sorted({name for keys in input_documents.values() for name in keys})
            artifact_values = await self.artifacts.get_many(
                [cast(str, documents[name].input.artifact) for name in requested]
            )
            preparations = TextPreparation(
                self.artifacts, self.preparer, known_artifacts=artifact_values
            )
            identities = {
                name: await preparations.preparation_identity(
                    ContentSelection(
                        artifact=artifact.ref, selection=documents[name].input.selector
                    )
                )
                for name, artifact in zip(requested, artifact_values, strict=True)
            }
            for document_key, keys in input_documents.items():
                inputs[document_key]["preparation"] = {item: identities[item] for item in keys}
            signatures = {key: content_signature(value) for key, value in inputs.items()}
            cached_by_signature: dict[str, list[dict[str, object]]] = {}
            signature_values = list(set(signatures.values()))
            async with self.database.read() as tx:
                for offset in range(0, len(signature_values), 400):
                    rows = (
                        await tx.connection.execute(
                            sa.select(
                                schema.projection_units.c.input_signature,
                                schema.projection_units.c.candidates,
                                schema.projection_units.c.candidate_data,
                            ).where(
                                schema.projection_units.c.input_signature.in_(
                                    signature_values[offset : offset + 400]
                                )
                            )
                        )
                    ).mappings()
                    for row in rows:
                        cached_by_signature.setdefault(
                            row["input_signature"],
                            [
                                {"mapping": candidate, **data}
                                for candidate, data in zip(
                                    row["candidates"], row["candidate_data"], strict=True
                                )
                            ],
                        )
            cached = {
                key: cached_by_signature[signature]
                for key, signature in signatures.items()
                if signature in cached_by_signature
            }
            frozen = await freeze_inputs(
                view, profile.space_id, self.artifacts, self.embedding, self.preparer, cached
            )
            component_signature = await self.execution.blocking(
                native_component_signature, frozen, profile
            )
            index_location = f"wave/components/{component_signature}"
            async with self._lifetime_lock:
                state.asset = index_location
            identity: dict[str, JsonValue] = {
                "snapshot_id": str(
                    uuid5(UUID(self.instance_id), f"component:{component_signature}")
                ),
                "space_signature": content_signature(
                    profile.space_definition.model_dump(mode="json")
                    if profile.space_definition
                    else None
                ),
                "build_config_signature": content_signature(profile.build_config),
                "component_signature": component_signature,
            }
            engine = self.engine()
            import nous_native as native

            token = native.CancellationToken()
            component = component_root / component_signature
            async with self._component_locks.hold(component_signature):
                component_state = await self.execution.blocking(self._component_state, component)
                if component_state == "incomplete":
                    await self._remove_incomplete_component(component, index_location)
                reused_component = component_state == "complete"
                started = perf_counter()
                if not reused_component:
                    async with AsyncExitStack() as base_lease:
                        previous = await self.reader.current(view.name)
                        base = None
                        if previous is not None and all(
                            previous.identity.get(key) == identity[key]
                            for key in ("space_signature", "build_config_signature")
                        ):
                            try:
                                base_row = await base_lease.enter_async_context(
                                    self.pin(previous.ref)
                                )
                            except NousError as error:
                                if error.code != "wave.snapshot_not_found":
                                    raise
                            else:
                                base, _ = await self.open(base_row)
                        await self.compute(
                            lambda: engine.build(
                                *bundles(frozen),
                                encode_json(identity),
                                str(component),
                                options_json=encode_json(profile.build_config),
                                base=base,
                                cancel=token,
                            ),
                            token,
                        )
                native_ms = (perf_counter() - started) * 1000
                reopened = await self.compute(
                    lambda: engine.open(str(component), encode_json(identity), cancel=token),
                    token,
                )
            statistics = json.loads(reopened.statistics_json)
            capabilities = list(reopened.capabilities)
            del reopened
            statistics["preparation"] = frozen.preparation_stats
            statistics["reused_native_component"] = reused_component
            statistics["stages_ms"] = {
                "content_prepare": frozen.preparation_stats["content_prepare_ms"],
                "embedding": frozen.preparation_stats["embedding_ms"],
                "native_build": native_ms,
            }
            mapping = frozen.mapping
            units: list[ProjectionUnit] = []
            candidate_vectors = {
                cast(str, vector["key"]): vector
                for vector in frozen.vector_mappings
                if vector["kind"] == "candidate"
            }
            rows_by_document: dict[str, list[tuple[int, CandidateMapping]]] = {}
            for index, candidate in enumerate(mapping["candidates"]):
                rows_by_document.setdefault(candidate["document"], []).append((index, candidate))
            for document_key, document in mapping["documents"].items():
                selected = rows_by_document.get(document_key, [])
                vector_mappings = [
                    candidate_vectors[candidate["key"]]
                    for _, candidate in selected
                    if candidate["key"] in candidate_vectors
                ]
                unit_artifacts = {document["artifact"]["id"]}
                input_roots = cast(dict[str, list[dict[str, str]]], inputs[document_key]["roots"])
                unit_artifacts.update(ref["id"] for refs in input_roots.values() for ref in refs)
                for _, candidate in selected:
                    if candidate["annotation"] is not None:
                        unit_artifacts.add(candidate["annotation"]["id"])
                    for field in ("content", "retrieval_content"):
                        unit_artifacts.add(
                            ContentSelection.model_validate(candidate[field]).artifact.id
                        )
                    unit_artifacts.update(
                        ContentSelection.model_validate(value).artifact.id
                        for value in candidate["origin_contents"]
                    )
                for vector in vector_mappings:
                    unit_artifacts.add(
                        cast(dict[str, dict[str, str]], vector["input"])["artifact"]["id"]
                    )
                unit_data = {
                    "input_signature": signatures[document_key],
                    "roots": inputs[document_key]["roots"],
                    "document_key": document_key,
                    "document": document,
                    "candidates": [candidate for _, candidate in selected],
                    "candidate_data": [frozen.candidate_data[index] for index, _ in selected],
                    "artifact_ids": sorted(unit_artifacts),
                    "vector_mappings": vector_mappings,
                }
                units.append(
                    ProjectionUnit.model_validate(
                        {
                            **unit_data,
                            "signature": content_signature(unit_data),
                            "candidate_rows": [index for index, _ in selected],
                        }
                    )
                )
            layout = {
                key: value
                for key, value in mapping.items()
                if key not in {"documents", "candidates"}
            }
            global_vectors = [
                item for item in frozen.vector_mappings if item["kind"] != "candidate"
            ]
            graph_payload = {
                "layout": {
                    key: layout[key] for key in ("concepts", "associations", "support_groups")
                },
                "vector_mappings": global_vectors,
                "artifact_ids": sorted(
                    {
                        cast(dict[str, dict[str, str]], vector["input"])["artifact"]["id"]
                        for vector in global_vectors
                    }
                ),
            }
            graph_signature = content_signature(graph_payload)
            prepared = SnapshotDraft.model_validate(
                {
                    "ref": {
                        "instance_id": self.instance_id,
                        "id": object_id,
                        "identifier": new_identifier("wav"),
                    },
                    "view": view,
                    "profile": profile,
                    "identity": identity,
                    "build_key": key,
                    "build_signature": build_signature,
                    "component_signature": component_signature,
                    "graph_signature": graph_signature,
                    "graph_payload": graph_payload,
                    "index_location": index_location,
                    "layout": layout,
                    "units": units,
                    "summary": {"capabilities": capabilities, "statistics": statistics},
                    "created_at": self.clock.utc_now().isoformat(),
                }
            )
            async with self.database.write() as tx:
                await self.registry.prepare_in(tx, prepared)
            get_logger(__name__).info(
                "wave.build.completed",
                extra={
                    "snapshot": prepared.ref.identifier,
                    "preparation": frozen.preparation_stats,
                    "stages_ms": statistics["stages_ms"],
                },
            )
            return PreparedSnapshot(
                ref=prepared.ref, build_signature=build_signature, summary=prepared.summary
            )

    @asynccontextmanager
    async def pin(self, ref: str | SnapshotRef) -> AsyncGenerator[SnapshotRow]:
        """Keep a snapshot registered while its caller uses its mapping or files."""
        async with self._lifetime_lock:
            identity = await self.reader.lookup(ref)
            if identity is None:
                raise NousError(
                    "wave.snapshot_not_found",
                    "Wave snapshot does not exist",
                    details={"ref": str(ref)},
                )
            object_id = identity["id"]
            self._borrowed[object_id] = self._borrowed.get(object_id, 0) + 1
        try:
            async with self._mapping_lock:
                if self._cached_mapping is not None and self._cached_mapping[0] == object_id:
                    row = self._cached_mapping[1]
                else:
                    exact = SnapshotRef(
                        instance_id=self.instance_id,
                        id=object_id,
                        identifier=identity["identifier"],
                    )
                    row = await self.resolve(exact)
                    self._cached_mapping = (object_id, row)
            yield row
        finally:

            async def unpin() -> None:
                async with self._lifetime_lock:
                    self._borrowed[object_id] -= 1
                    if self._borrowed[object_id] == 0:
                        del self._borrowed[object_id]

            await complete_cleanup(unpin())

    @asynccontextmanager
    async def borrow(
        self, ref: str | SnapshotRef
    ) -> AsyncGenerator[
        tuple[SnapshotRow, nous_native.NativeSnapshot, nous_native.CancellationToken]
    ]:
        """Hold the Native reader and its persistent inputs throughout a query."""
        async with self.pin(ref) as row:
            snapshot, token = await self.open(row)
            yield row, snapshot, token

    async def cleanup(
        self, *, keep_recent: int = 0, retain: Sequence[SnapshotRef] = ()
    ) -> dict[str, object]:
        """Remove eligible snapshots and retry deletion of unreferenced components."""
        if keep_recent < 0:
            raise NousError("wave.invalid_retention", "keep_recent must be nonnegative")
        root = self.paths.resolve_owned("index", "wave/components")
        async with self._lifetime_lock:
            pending_assets = {state.asset for state in self._prepared.values() if state.asset}
            async with self.database.write() as tx:
                selection = await self.registry.remove_unreferenced_in(
                    tx,
                    keep_recent=keep_recent,
                    retain=retain,
                    protected=set(self._borrowed) | set(self._prepared),
                    pending_assets=pending_assets,
                )
            removed = selection.removed
            removed_ids = {item[0] for item in removed}
            if self._cached_snapshot and self._cached_snapshot[0] in removed_ids:
                self._cached_snapshot = None
            if self._cached_mapping and self._cached_mapping[0] in removed_ids:
                self._cached_mapping = None
            deleted: list[str] = []
            if await self.execution.blocking(root.exists):
                active = {
                    Path(value).name for value in pending_assets
                } | self._component_locks.active_keys()
                for path in await self.execution.blocking(self._entries, root):
                    name = path.name
                    removable = (
                        len(name) == 64
                        and all(char in "0123456789abcdef" for char in name)
                        and name not in active
                        and f"wave/components/{name}" not in selection.remaining_assets
                    )
                    if removable:
                        await self.execution.blocking(self._remove_owned, path, root, name)
                        deleted.append(name)
        return {"snapshots": [identifier for _, identifier in removed], "staging": deleted}

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
