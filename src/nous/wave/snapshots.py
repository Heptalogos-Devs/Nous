"""Immutable snapshot construction, publication, identity and Native resource lifetime."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
from collections.abc import AsyncGenerator, Callable, Sequence
from contextlib import AsyncExitStack, asynccontextmanager
from pathlib import Path
from time import perf_counter
from typing import TYPE_CHECKING, cast
from uuid import UUID, uuid5

import sqlalchemy as sa
from pydantic import JsonValue

from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.common.identifier import new_identifier
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
    SnapshotRef,
)
from .importer import ManifestImporter
from .keyed_locks import KeyedLocks
from .manifests import (
    content_signature,
    encode_json,
    manifest_references,
    manifest_value,
    restore_prepared,
    write_shared,
    write_units,
)
from .mapping import CandidateMapping, SnapshotRow
from .native_inputs import bundles
from .native_inputs import component_signature as native_component_signature
from .projection import projection_inputs
from .reading import SnapshotReader
from .registry import SnapshotRegistry

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
        self._build_locks = KeyedLocks()
        self._component_locks = KeyedLocks()
        self._unit_locks = KeyedLocks()
        self._cached_snapshot: tuple[str, nous_native.NativeSnapshot] | None = None
        self._cached_mapping: tuple[str, SnapshotRow] | None = None
        self._mapping_lock = asyncio.Lock()
        self._open_lock = asyncio.Lock()
        self._lifetime_lock = asyncio.Lock()
        self._borrowed: dict[str, int] = {}
        self._prepared_counts: dict[str, int] = {}
        self._prepared_ready: dict[str, int] = {}
        self._prepared_assets: dict[str, str] = {}
        self._prepared_units: dict[str, set[str]] = {}
        self._prepared_graphs: dict[str, str] = {}

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
                        ).select_from(schema.snapshots.join(schema.projection_graphs))
                    )
                )
                .mappings()
                .one()
            )
            collections = await tx.connection.scalar(
                sa.select(sa.func.count()).select_from(schema.collections)
            )
            candidates = await tx.connection.scalar(
                sa.select(sa.func.count()).select_from(schema.snapshot_candidates)
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
        """Discard an interrupted build only when no accepted or prepared result owns it."""
        async with self._lifetime_lock:
            async with self.database.read() as tx:
                referenced = await tx.connection.scalar(
                    sa.select(sa.exists().where(schema.snapshots.c.index_location == location))
                )
            prepared = any(
                self._prepared_ready.get(object_id, 0) > 0 and asset == location
                for object_id, asset in self._prepared_assets.items()
            )
            manifest_root = self.paths.resolve_owned("index", "wave/snapshots")
            if (
                not referenced
                and not prepared
                and await self.execution.blocking(manifest_root.exists)
            ):
                for entry in await self.execution.blocking(self._entries, manifest_root):
                    if entry.name.startswith(".staging-"):
                        continue
                    asset, _, _ = await self.execution.blocking(manifest_references, entry)
                    if asset == location:
                        prepared = True
                        break
            if referenced or prepared:
                raise NousError(
                    "wave.snapshot_invalid",
                    "An owned Native component has no complete manifest",
                    details={"location": location},
                )
            await self.execution.blocking(self._remove_owned, path, path.parent, path.name)

    async def prepare_build(
        self,
        view: CorpusView,
        profile: ResolvedBuildProfile,
        *,
        build_key: str | None = None,
    ) -> PreparedSnapshot:
        signature = content_signature(
            {"view": view.model_dump(mode="json"), "profile": profile.model_dump(mode="json")}
        )
        object_id = str(uuid5(UUID(self.instance_id), build_key or f"view:{signature}"))
        self._prepared_counts[object_id] = self._prepared_counts.get(object_id, 0) + 1
        try:
            prepared = await self._prepare_build(view, profile, build_key=build_key)
            self._prepared_ready[object_id] = self._prepared_ready.get(object_id, 0) + 1
            return prepared
        except BaseException:
            self._release_prepared_id(object_id)
            raise

    async def _prepare_build(
        self,
        view: CorpusView,
        profile: ResolvedBuildProfile,
        *,
        build_key: str | None = None,
    ) -> PreparedSnapshot:
        """Construct immutable files and a recoverable manifest outside a transaction."""
        await self.time.validate(document.times for document in view.documents)
        if view.space is not None and view.space != profile.space_id:
            raise NousError("wave.space_conflict", "Manifest and build select different spaces")
        build_signature = content_signature(
            {
                "view": view.model_dump(mode="json"),
                "profile": profile.model_dump(mode="json"),
            }
        )
        key = build_key or f"view:{build_signature}"
        object_id = str(uuid5(UUID(self.instance_id), key))
        root = self.paths.resolve_owned("index", "wave")
        manifest_root, component_root, unit_root = (
            root / "snapshots",
            root / "components",
            root / "units",
        )
        await self.execution.blocking(manifest_root.mkdir, parents=True, exist_ok=True)
        await self.execution.blocking(component_root.mkdir, parents=True, exist_ok=True)
        await self.execution.blocking(unit_root.mkdir, parents=True, exist_ok=True)
        staging_name = f".staging-{object_id}"
        staging, final = manifest_root / staging_name, manifest_root / object_id
        async with self._build_locks.hold(key):
            if await self.execution.blocking(final.exists):

                def restore() -> PreparedSnapshot:
                    restored = restore_prepared(final, unit_root)
                    if restored.build_signature != build_signature or restored.build_key != key:
                        raise NousError(
                            "wave.build_key_conflict",
                            "Build key identifies a different captured view",
                        )
                    asset = self.paths.resolve_owned("index", restored.index_location)
                    native_manifest = json.loads(
                        (asset / "manifest.json").read_text(encoding="utf-8")
                    )
                    if native_manifest != restored.native_manifest:
                        raise NousError(
                            "wave.manifest_invalid", "Native and prepared manifests differ"
                        )
                    return restored

                async with self._lifetime_lock:
                    prepared = await self.execution.blocking(restore)
                    self._prepared_assets[object_id] = prepared.index_location
                    self._prepared_units[object_id] = {unit.signature for unit in prepared.units}
                    self._prepared_graphs[object_id] = prepared.graph_signature
                import nous_native as native

                token = native.CancellationToken()
                asset = self.paths.resolve_owned("index", prepared.index_location)
                await self.compute(
                    lambda: self.engine().open(
                        str(asset), encode_json(prepared.identity), cancel=token
                    ),
                    token,
                )
                return prepared

            if await self.execution.blocking(staging.exists):
                await self.execution.blocking(
                    self._remove_owned, staging, manifest_root, staging_name
                )
            inputs = projection_inputs(view)
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
                self._prepared_assets[object_id] = index_location
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
            published = False
            try:
                await self.execution.blocking(staging.mkdir, parents=True, exist_ok=False)
                component = component_root / component_signature
                async with self._component_locks.hold(component_signature):
                    state = await self.execution.blocking(self._component_state, component)
                    if state == "incomplete":
                        await self._remove_incomplete_component(component, index_location)
                    reused_component = state == "complete"
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
                            built = await self.compute(
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
                        native_manifest = json.loads(built)["manifest"]
                    else:
                        native_manifest = json.loads(
                            await self.execution.blocking(
                                (component / "manifest.json").read_text, encoding="utf-8"
                            )
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
                    rows_by_document.setdefault(candidate["document"], []).append(
                        (index, candidate)
                    )
                for document_key, document in mapping["documents"].items():
                    selected = rows_by_document.get(document_key, [])
                    vector_mappings = [
                        candidate_vectors[candidate["key"]]
                        for _, candidate in selected
                        if candidate["key"] in candidate_vectors
                    ]
                    unit_artifacts = {document["artifact"]["id"]}
                    input_roots = cast(
                        dict[str, list[dict[str, str]]], inputs[document_key]["roots"]
                    )
                    unit_artifacts.update(
                        ref["id"] for refs in input_roots.values() for ref in refs
                    )
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
                        "input_payload": inputs[document_key],
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
                input_refs: set[str] = (
                    {view.manifest_artifact.id} if view.manifest_artifact else set()
                )
                global_vectors = [
                    item for item in frozen.vector_mappings if item["kind"] != "candidate"
                ]
                graph_payload = {
                    "view": {
                        "concepts": [item.model_dump(mode="json") for item in view.concepts],
                        "associations": [
                            item.model_dump(mode="json") for item in view.associations
                        ],
                        "support_groups": [
                            item.model_dump(mode="json") for item in view.support_groups
                        ],
                    },
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
                prepared = PreparedSnapshot.model_validate(
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
                        "vector_mappings": frozen.vector_mappings,
                        "artifact_ids": sorted(input_refs),
                        "native_manifest": native_manifest,
                        "summary": {"capabilities": capabilities, "statistics": statistics},
                        "created_at": self.clock.utc_now().isoformat(),
                    }
                )
                async with self._lifetime_lock:
                    self._prepared_units[object_id] = {unit.signature for unit in prepared.units}
                    self._prepared_graphs[object_id] = prepared.graph_signature
                async with self._unit_locks.hold(f"graph-{prepared.graph_signature}"):
                    await self.execution.blocking(
                        write_shared,
                        unit_root,
                        prepared.graph_signature,
                        prepared.graph_payload,
                        prefix="graph-",
                    )
                for offset in range(0, len(prepared.units), 64):
                    batch = prepared.units[offset : offset + 64]
                    async with AsyncExitStack() as unit_leases:
                        for signature in sorted({unit.signature for unit in batch}):
                            await unit_leases.enter_async_context(self._unit_locks.hold(signature))
                        await self.execution.blocking(write_units, unit_root, batch)
                value = manifest_value(prepared)
                envelope = {"prepared": value, "digest": content_signature(value)}
                await self.execution.blocking(
                    (staging / "wave-manifest.json").write_text,
                    encode_json(envelope),
                    encoding="utf-8",
                )
                await self.execution.blocking(os.replace, staging, final)
                published = True
                get_logger(__name__).info(
                    "wave.build.completed",
                    extra={
                        "snapshot": prepared.ref.identifier,
                        "preparation": frozen.preparation_stats,
                        "stages_ms": statistics["stages_ms"],
                    },
                )
                return prepared
            except BaseException:
                if not published and await self.execution.blocking(staging.exists):
                    await self.execution.blocking(
                        self._remove_owned, staging, manifest_root, staging_name
                    )
                raise

    def release_prepared(self, prepared: PreparedSnapshot) -> None:
        """Release temporary ownership after the caller commits or abandons the result."""
        ready = self._prepared_ready.get(prepared.ref.id, 0)
        if ready > 1:
            self._prepared_ready[prepared.ref.id] = ready - 1
        else:
            self._prepared_ready.pop(prepared.ref.id, None)
        self._release_prepared_id(prepared.ref.id)

    def _release_prepared_id(self, object_id: str) -> None:
        count = self._prepared_counts.get(object_id, 0)
        if count > 1:
            self._prepared_counts[object_id] = count - 1
        else:
            self._prepared_counts.pop(object_id, None)
            self._prepared_assets.pop(object_id, None)
            self._prepared_units.pop(object_id, None)
            self._prepared_graphs.pop(object_id, None)

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
        """Remove eligible snapshots and unreferenced staging directories on request."""
        if keep_recent < 0:
            raise NousError("wave.invalid_retention", "keep_recent must be nonnegative")
        root = self.paths.resolve_owned("index", "wave")
        manifest_root, component_root, unit_root = (
            root / "snapshots",
            root / "components",
            root / "units",
        )
        removed: list[tuple[str, str]] = []
        async with self._lifetime_lock:
            async with self.database.read() as tx:
                registered = set(
                    (await tx.connection.scalars(sa.select(schema.snapshots.c.id))).all()
                )
            pending_assets: set[str] = set(self._prepared_assets.values())
            pending_units: set[str] = set()
            pending_graphs = set(self._prepared_graphs.values())
            for units in self._prepared_units.values():
                pending_units.update(units)
            if await self.execution.blocking(manifest_root.exists):
                entries = await self.execution.blocking(lambda: list(manifest_root.iterdir()))
                for path in entries:
                    if path.name in registered or path.name.startswith(".staging-"):
                        continue

                    asset, units, graph = await self.execution.blocking(manifest_references, path)
                    pending_assets.add(asset)
                    pending_units.update(units)
                    pending_graphs.add(graph)
            async with self.database.write() as tx:
                selection = await self.registry.remove_unreferenced_in(
                    tx,
                    keep_recent=keep_recent,
                    retain=retain,
                    protected=set(self._borrowed) | set(self._prepared_counts),
                    pending_assets=pending_assets,
                )
            removed = selection.removed
            remaining_assets, removed_assets = selection.remaining_assets, selection.removed_assets
            unused_units, unused_graphs = selection.unused_units, selection.unused_graphs
            removed_ids = {item[0] for item in removed}
            if self._cached_snapshot and self._cached_snapshot[0] in removed_ids:
                self._cached_snapshot = None
            if self._cached_mapping and self._cached_mapping[0] in removed_ids:
                self._cached_mapping = None
            for object_id, _ in removed:
                await self.execution.blocking(
                    self._remove_owned, manifest_root / object_id, manifest_root, object_id
                )
            for location in removed_assets:
                path = self.paths.resolve_owned("index", location)
                await self.execution.blocking(self._remove_owned, path, component_root, path.name)
            for signature in unused_units - pending_units:
                unit = unit_root / f"{signature}.json"
                if await self.execution.blocking(unit.exists):
                    await self.execution.blocking(unit.unlink)
            for signature in unused_graphs - pending_graphs:
                graph = unit_root / f"graph-{signature}.json"
                if await self.execution.blocking(graph.exists):
                    await self.execution.blocking(graph.unlink)
            staging_removed: list[str] = []
            active_components = {Path(value).name for value in self._prepared_assets.values()}
            for directory, active in (
                (manifest_root, set(self._prepared_counts)),
                (component_root, active_components),
            ):
                if await self.execution.blocking(directory.exists):
                    entries = await self.execution.blocking(self._entries, directory)
                    for path in entries:
                        if (
                            path.name.startswith(".staging-")
                            and path.name.removeprefix(".staging-") not in active
                        ):
                            await self.execution.blocking(
                                self._remove_owned, path, directory, path.name
                            )
                            staging_removed.append(path.name)
                        elif (
                            directory == component_root
                            and len(path.name) == 64
                            and all(char in "0123456789abcdef" for char in path.name)
                            and f"wave/components/{path.name}" not in remaining_assets
                            and await self.execution.blocking(self._component_state, path)
                            == "incomplete"
                        ):
                            await self.execution.blocking(
                                self._remove_owned, path, directory, path.name
                            )
                            staging_removed.append(path.name)
            if await self.execution.blocking(unit_root.exists):
                entries = await self.execution.blocking(self._entries, unit_root)
                for path in entries:
                    name = path.name.removeprefix(".staging-")
                    active = (
                        name[6:70] in pending_graphs
                        if name.startswith("graph-")
                        else name[:64] in pending_units
                    )
                    if path.name.startswith(".staging-") and not active:
                        await self.execution.blocking(path.unlink)
                        staging_removed.append(path.name)
        return {"snapshots": [identifier for _, identifier in removed], "staging": staging_removed}

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
