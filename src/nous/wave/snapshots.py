"""Immutable snapshot construction, publication, identity and Native resource lifetime."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shutil
from collections.abc import AsyncGenerator, Callable, Sequence
from contextlib import asynccontextmanager
from dataclasses import fields
from pathlib import Path
from time import perf_counter
from typing import TYPE_CHECKING, cast
from uuid import UUID, uuid4, uuid5

import numpy as np
import numpy.typing as npt
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
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database, WriteTx
from nous.temporal.domains import TimeService

from . import schema
from .assets import (
    CandidateMapping,
    FrozenInputs,
    SnapshotRow,
    content_evidence,
    freeze_inputs,
    projection_inputs,
)
from .configuration import require_native_profiles
from .contracts import (
    CollectionManifest,
    CorpusView,
    Document,
    DocumentInput,
    PreparedSnapshot,
    ProjectionUnit,
    ResolvedBuildProfile,
    SnapshotDescriptor,
    SnapshotOwner,
    SnapshotRef,
)

if TYPE_CHECKING:
    import nous_native


def encode_json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _signature(value: object) -> str:
    return hashlib.sha256(encode_json(value).encode("utf-8")).hexdigest()


def _component_signature(frozen: FrozenInputs, profile: ResolvedBuildProfile) -> str:
    """Hash every Native input, including its stable candidate-to-row layout."""
    digest = hashlib.sha256(encode_json(profile.model_dump(mode="json")).encode())
    for field in fields(frozen):
        if field.name in {"preparation_stats", "mapping", "vector_mappings", "candidate_data"}:
            continue
        value = getattr(frozen, field.name)
        digest.update(field.name.encode())
        if isinstance(value, np.ndarray):
            array = cast(npt.NDArray[np.generic], value)
            digest.update(encode_json({"dtype": array.dtype.str, "shape": array.shape}).encode())
            digest.update(array.tobytes(order="C"))
        else:
            digest.update(encode_json(value).encode())
    return digest.hexdigest()


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
        self._component_locks: dict[str, asyncio.Lock] = {}
        self._unit_locks: dict[str, asyncio.Lock] = {}
        self._cached_snapshot: tuple[str, nous_native.NativeSnapshot] | None = None
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
                    asset, _, _ = await self.execution.blocking(self._manifest_references, entry)
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

    @staticmethod
    def _manifest_references(path: Path) -> tuple[str, set[str], str]:
        envelope = json.loads((path / "wave-manifest.json").read_text(encoding="utf-8"))
        value = envelope["prepared"]
        if envelope["digest"] != _signature(value):
            raise NousError("wave.manifest_invalid", "Prepared manifest checksum differs")
        return (
            value["index_location"],
            {item["signature"] for item in value["unit_refs"]},
            value["graph_signature"],
        )

    @staticmethod
    def _write_unit(root: Path, unit: ProjectionUnit) -> None:
        data = unit.model_dump(mode="json", exclude={"signature", "candidate_rows"})
        WaveSnapshots._write_shared(root, unit.signature, data, prefix="")

    @staticmethod
    def _write_shared(root: Path, signature: str, data: object, *, prefix: str) -> None:
        if _signature(data) != signature:
            raise NousError("wave.manifest_invalid", "Projection asset signature differs")
        path = root / f"{prefix}{signature}.json"
        if path.exists():
            if _signature(json.loads(path.read_text(encoding="utf-8"))) != signature:
                raise NousError("wave.manifest_invalid", "Stored projection asset differs")
            return
        staging = root / f".staging-{prefix}{signature}-{uuid4()}"
        try:
            staging.write_text(encode_json(data), encoding="utf-8")
            os.replace(staging, path)
        finally:
            if staging.exists():
                staging.unlink()

    @staticmethod
    def _manifest_value(prepared: PreparedSnapshot) -> dict[str, object]:
        data = prepared.model_dump(
            mode="json", exclude={"units", "graph_payload", "vector_mappings"}
        )
        view = data["view"]
        documents = view.pop("documents")
        candidates = view.pop("candidates")
        evidence = view.pop("evidence")
        data["source_documents"] = evidence["source_documents"]
        data["sequence_documents"] = [item["document"] for item in view.pop("source_sequences")]
        for name in ("concepts", "associations", "support_groups"):
            view.pop(name)
        layout = data["layout"]
        for name in (
            "name",
            "owner",
            "metadata",
            "roots",
            "concepts",
            "associations",
            "support_groups",
        ):
            layout.pop(name)
        data["document_order"] = [item["key"] for item in documents]
        data["candidate_order"] = [
            item["key"] if item["kind"] == "exact" else item["group_key"] for item in candidates
        ]
        data["unit_refs"] = [
            {"signature": unit.signature, "candidate_rows": list(unit.candidate_rows)}
            for unit in prepared.units
        ]
        return data

    @staticmethod
    def _restore_prepared(path: Path, unit_root: Path) -> PreparedSnapshot:
        envelope = json.loads((path / "wave-manifest.json").read_text(encoding="utf-8"))
        data = envelope["prepared"]
        if envelope["digest"] != _signature(data):
            raise NousError("wave.manifest_invalid", "Prepared manifest checksum differs")
        references = data.pop("unit_refs")
        document_order = data.pop("document_order")
        candidate_order = data.pop("candidate_order")
        source_documents = data.pop("source_documents")
        sequence_documents = data.pop("sequence_documents")
        graph = json.loads(
            (unit_root / f"graph-{data['graph_signature']}.json").read_text(encoding="utf-8")
        )
        if _signature(graph) != data["graph_signature"]:
            raise NousError("wave.manifest_invalid", "Projection graph checksum differs")
        units: list[ProjectionUnit] = []
        documents: dict[str, object] = {}
        candidates: dict[str, object] = {}
        roots: dict[str, object] = {}
        document_roots: dict[str, object] = {}
        candidate_vectors: dict[str, dict[str, JsonValue]] = {}
        candidate_rows: list[tuple[int, str]] = []
        sequences: dict[str, list[dict[str, JsonValue]]] = {}
        for reference in references:
            signature = reference["signature"]
            unit_data = json.loads((unit_root / f"{signature}.json").read_text(encoding="utf-8"))
            if _signature(unit_data) != signature:
                raise NousError("wave.manifest_invalid", "Projection unit checksum differs")
            unit = ProjectionUnit.model_validate({**unit_data, **reference})
            if _signature(unit.input_payload) != unit.input_signature:
                raise NousError("wave.manifest_invalid", "Projection input signature differs")
            units.append(unit)
            for vector in unit.vector_mappings:
                candidate_vectors[cast(str, vector["key"])] = vector
            candidate_rows.extend(
                (row, cast(str, candidate["key"]))
                for row, candidate in zip(unit.candidate_rows, unit.candidates, strict=True)
            )
            roots.update(
                TypeAdapter(dict[str, JsonValue]).validate_python(unit.input_payload["roots"])
            )
            document_roots.update(
                TypeAdapter(dict[str, JsonValue]).validate_python(
                    unit.input_payload["document_roots"]
                )
            )
            for item in TypeAdapter(list[dict[str, JsonValue]]).validate_python(
                unit.input_payload["documents"]
            ):
                documents[cast(str, item["key"])] = item
            for item in TypeAdapter(list[dict[str, JsonValue]]).validate_python(
                unit.input_payload["declarations"]
            ):
                key = item["key"] if item["kind"] == "exact" else item["group_key"]
                candidates[cast(str, key)] = item
            sequences[unit.document_key] = [
                item
                for item in TypeAdapter(list[dict[str, JsonValue]]).validate_python(
                    unit.input_payload["sequences"]
                )
                if item["document"] == unit.document_key
            ]
        data["view"]["documents"] = [documents[key] for key in document_order]
        data["view"]["candidates"] = [candidates[key] for key in candidate_order]
        positions: dict[str, int] = {}
        source_sequences: list[dict[str, JsonValue]] = []
        for document in sequence_documents:
            index = positions.get(document, 0)
            source_sequences.append(sequences[document][index])
            positions[document] = index + 1
        data["view"]["source_sequences"] = source_sequences
        data["view"].update(graph["view"])
        data["view"]["evidence"] = {
            "roots": roots,
            "documents": document_roots,
            "source_documents": source_documents,
        }
        data["layout"].update(
            {
                "name": data["view"]["name"],
                "owner": data["view"]["owner"],
                "metadata": data["view"]["metadata"],
                "roots": roots,
                **graph["layout"],
            }
        )
        data["vector_mappings"] = [
            candidate_vectors[key] for _, key in sorted(candidate_rows) if key in candidate_vectors
        ] + graph["vector_mappings"]
        data["graph_payload"] = graph
        data["units"] = units
        prepared = PreparedSnapshot.model_validate(data)
        if (
            _signature(
                {
                    "view": prepared.view.model_dump(mode="json"),
                    "profile": prepared.profile.model_dump(mode="json"),
                }
            )
            != prepared.build_signature
        ):
            raise NousError("wave.manifest_invalid", "Prepared input signature differs")
        if (
            prepared.identity.get("component_signature") != prepared.component_signature
            or prepared.index_location != f"wave/components/{prepared.component_signature}"
            or [row for row, _ in sorted(candidate_rows)] != list(range(len(candidate_rows)))
        ):
            raise NousError("wave.manifest_invalid", "Prepared component or row mapping differs")
        return prepared

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

    async def prepare_build(
        self,
        view: CorpusView,
        profile: ResolvedBuildProfile,
        *,
        build_key: str | None = None,
    ) -> PreparedSnapshot:
        signature = _signature(
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
        build_signature = _signature(
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
        async with self._build_locks.setdefault(key, asyncio.Lock()):
            if await self.execution.blocking(final.exists):

                def restore() -> PreparedSnapshot:
                    restored = self._restore_prepared(final, unit_root)
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
            signatures = {key: _signature(value) for key, value in inputs.items()}
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
                _component_signature, frozen, profile
            )
            index_location = f"wave/components/{component_signature}"
            async with self._lifetime_lock:
                self._prepared_assets[object_id] = index_location
            identity: dict[str, JsonValue] = {
                "snapshot_id": str(
                    uuid5(UUID(self.instance_id), f"component:{component_signature}")
                ),
                "space_signature": _signature(
                    profile.space_definition.model_dump(mode="json")
                    if profile.space_definition
                    else None
                ),
                "build_config_signature": _signature(profile.build_config),
                "component_signature": component_signature,
            }
            engine = self.engine()
            import nous_native as native

            token = native.CancellationToken()
            published = False
            try:
                await self.execution.blocking(staging.mkdir, parents=True, exist_ok=False)
                component = component_root / component_signature
                async with self._component_locks.setdefault(component_signature, asyncio.Lock()):
                    state = await self.execution.blocking(self._component_state, component)
                    if state == "incomplete":
                        await self._remove_incomplete_component(component, index_location)
                    reused_component = state == "complete"
                    started = perf_counter()
                    if not reused_component:
                        built = await self.compute(
                            lambda: engine.build(
                                *self._bundles(frozen),
                                encode_json(identity),
                                str(component),
                                options_json=encode_json(profile.build_config),
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
                                "signature": _signature(unit_data),
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
                graph_signature = _signature(graph_payload)
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
                async with self._unit_locks.setdefault(
                    f"graph-{prepared.graph_signature}", asyncio.Lock()
                ):
                    await self.execution.blocking(
                        self._write_shared,
                        unit_root,
                        prepared.graph_signature,
                        prepared.graph_payload,
                        prefix="graph-",
                    )
                for unit in prepared.units:
                    async with self._unit_locks.setdefault(unit.signature, asyncio.Lock()):
                        await self.execution.blocking(self._write_unit, unit_root, unit)
                value = self._manifest_value(prepared)
                envelope = {"prepared": value, "digest": _signature(value)}
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

    async def accept_in(self, tx: WriteTx, prepared: PreparedSnapshot) -> SnapshotDescriptor:
        """Register exactly the prepared files and their dependencies in the caller's transaction."""
        if prepared.ref.instance_id != self.instance_id:
            raise NousError(
                "wave.instance_mismatch", "Prepared snapshot belongs to another instance"
            )
        previous = (
            (
                await tx.connection.execute(
                    sa.select(schema.snapshots, schema.collections.c.name)
                    .join(
                        schema.collections,
                        schema.snapshots.c.collection_id == schema.collections.c.id,
                    )
                    .where(schema.snapshots.c.id == prepared.ref.id)
                )
            )
            .mappings()
            .one_or_none()
        )
        if previous is not None:
            if (
                previous["build_signature"] != prepared.build_signature
                or previous["identifier"] != prepared.ref.identifier
            ):
                raise NousError(
                    "wave.build_key_conflict", "Build key identifies a different captured view"
                )
            return self._descriptor(previous)
        await tx.connection.execute(
            insert(schema.collections)
            .values(id=str(uuid4()), name=prepared.view.name)
            .on_conflict_do_nothing(index_elements=[schema.collections.c.name])
        )
        collection_id = await tx.connection.scalar(
            sa.select(schema.collections.c.id).where(
                schema.collections.c.name == prepared.view.name
            )
        )
        if collection_id is None:
            raise NousError("wave.collection_unavailable", "Cannot create collection")
        graph_layout = TypeAdapter(dict[str, JsonValue]).validate_python(
            prepared.graph_payload["layout"]
        )
        await tx.connection.execute(
            insert(schema.projection_graphs)
            .values(
                signature=prepared.graph_signature,
                concepts=graph_layout["concepts"],
                associations=graph_layout["associations"],
                support_groups=graph_layout["support_groups"],
            )
            .on_conflict_do_nothing(index_elements=[schema.projection_graphs.c.signature])
        )
        graph_artifacts = TypeAdapter(list[str]).validate_python(
            prepared.graph_payload["artifact_ids"]
        )
        if graph_artifacts:
            await tx.connection.execute(
                insert(schema.projection_graph_artifacts).on_conflict_do_nothing(),
                [
                    {"graph_signature": prepared.graph_signature, "artifact_id": ref}
                    for ref in graph_artifacts
                ],
            )
        graph_vectors = TypeAdapter(list[dict[str, JsonValue]]).validate_python(
            prepared.graph_payload["vector_mappings"]
        )
        if graph_vectors:
            await tx.connection.execute(
                insert(schema.projection_graph_vectors).on_conflict_do_nothing(),
                [
                    {
                        "graph_signature": prepared.graph_signature,
                        "key": vector["key"],
                        "input": vector["input"],
                        "result_id": vector["result_id"],
                        "row_index": vector["row_index"],
                    }
                    for vector in graph_vectors
                ],
            )
        await tx.connection.execute(
            schema.snapshots.insert().values(
                id=prepared.ref.id,
                identifier=prepared.ref.identifier,
                collection_id=collection_id,
                manifest_artifact_id=prepared.view.manifest_artifact.id
                if prepared.view.manifest_artifact
                else None,
                covered_generation=prepared.view.generation,
                profile=prepared.view.profile,
                space_id=prepared.profile.space_id,
                space_definition=prepared.profile.space_definition.model_dump(mode="json")
                if prepared.profile.space_definition
                else None,
                identity=prepared.identity,
                index_location=prepared.index_location,
                layout={
                    key: value
                    for key, value in prepared.layout.items()
                    if key
                    not in {
                        "name",
                        "owner",
                        "metadata",
                        "roots",
                        "concepts",
                        "associations",
                        "support_groups",
                    }
                },
                owner=prepared.view.owner.model_dump(mode="json"),
                metadata=prepared.view.metadata,
                build_key=prepared.build_key,
                build_signature=prepared.build_signature,
                component_signature=prepared.component_signature,
                graph_signature=prepared.graph_signature,
                build_summary=prepared.summary,
                created_at=prepared.created_at,
            )
        )
        if prepared.units:
            await tx.connection.execute(
                insert(schema.projection_units).on_conflict_do_nothing(
                    index_elements=[schema.projection_units.c.signature]
                ),
                [
                    {
                        "signature": unit.signature,
                        "input_signature": unit.input_signature,
                        "document_key": unit.document_key,
                        "document": unit.document,
                        "roots": unit.input_payload["roots"],
                        "candidates": unit.candidates,
                        "candidate_data": unit.candidate_data,
                    }
                    for unit in prepared.units
                ],
            )
            await tx.connection.execute(
                schema.snapshot_members.insert(),
                [
                    {
                        "snapshot_id": prepared.ref.id,
                        "ordinal": ordinal,
                        "unit_signature": unit.signature,
                    }
                    for ordinal, unit in enumerate(prepared.units)
                ],
            )
            candidates = [
                {
                    "snapshot_id": prepared.ref.id,
                    "key": candidate["key"],
                    "unit_signature": unit.signature,
                    "unit_index": index,
                    "row_index": row,
                }
                for unit in prepared.units
                for index, (candidate, row) in enumerate(
                    zip(unit.candidates, unit.candidate_rows, strict=True)
                )
            ]
            if candidates:
                await tx.connection.execute(schema.snapshot_candidates.insert(), candidates)
            unit_artifacts = [
                {"unit_signature": unit.signature, "artifact_id": artifact_id}
                for unit in prepared.units
                for artifact_id in unit.artifact_ids
            ]
            if unit_artifacts:
                await tx.connection.execute(
                    insert(schema.projection_artifacts).on_conflict_do_nothing(), unit_artifacts
                )
            unit_vectors = [
                {
                    "unit_signature": unit.signature,
                    "key": vector["key"],
                    "input": vector["input"],
                    "result_id": vector["result_id"],
                    "row_index": vector["row_index"],
                }
                for unit in prepared.units
                for vector in unit.vector_mappings
            ]
            if unit_vectors:
                await tx.connection.execute(
                    insert(schema.projection_vectors).on_conflict_do_nothing(), unit_vectors
                )
        if prepared.artifact_ids:
            await tx.connection.execute(
                schema.snapshot_artifacts.insert(),
                [
                    {"snapshot_id": prepared.ref.id, "artifact_id": ref}
                    for ref in prepared.artifact_ids
                ],
            )
        return SnapshotDescriptor(
            ref=prepared.ref,
            owner=prepared.view.owner,
            collection=prepared.view.name,
            captured_generation=prepared.view.generation,
            profile_signature=prepared.view.profile,
            space_id=prepared.profile.space_id,
            space_signature=cast(str, prepared.identity["space_signature"]),
            identity=prepared.identity,
            metadata=prepared.view.metadata,
        )

    async def promote_in(self, tx: WriteTx, ref: SnapshotRef) -> None:
        """Point the owning collection to an already accepted snapshot."""
        if ref.instance_id != self.instance_id:
            raise NousError("wave.instance_mismatch", "Snapshot belongs to another instance")
        snapshot = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.snapshots.c.collection_id, schema.snapshots.c.identifier
                    ).where(schema.snapshots.c.id == ref.id)
                )
            )
            .mappings()
            .one_or_none()
        )
        if snapshot is None:
            raise NousError("wave.snapshot_not_found", "Wave snapshot does not exist")
        if snapshot["identifier"] != ref.identifier:
            raise NousError(
                "wave.snapshot_mismatch", "Snapshot reference differs from its identity"
            )
        await tx.connection.execute(
            schema.collections.update()
            .where(schema.collections.c.id == snapshot["collection_id"])
            .values(current_snapshot_id=ref.id)
        )

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

    async def retain_in(
        self, tx: WriteTx, ref: SnapshotRef, *, owner_kind: str, owner_id: str
    ) -> None:
        """Persist a formal owner's dependency on an exact snapshot."""
        if ref.instance_id != self.instance_id or not owner_kind or not owner_id:
            raise NousError("wave.invalid_reference", "Invalid snapshot retention reference")
        identifier = await tx.connection.scalar(
            sa.select(schema.snapshots.c.identifier).where(schema.snapshots.c.id == ref.id)
        )
        if identifier != ref.identifier:
            raise NousError("wave.snapshot_not_found", "Snapshot is no longer available")
        await tx.connection.execute(
            insert(schema.snapshot_references)
            .values(snapshot_id=ref.id, owner_kind=owner_kind, owner_id=owner_id)
            .on_conflict_do_nothing()
        )

    async def release_in(self, tx: WriteTx, *, owner_kind: str, owner_id: str) -> None:
        await tx.connection.execute(
            schema.snapshot_references.delete().where(
                schema.snapshot_references.c.owner_kind == owner_kind,
                schema.snapshot_references.c.owner_id == owner_id,
            )
        )

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

    async def candidates(
        self, ref: SnapshotRef, keys: Sequence[str]
    ) -> dict[str, CandidateMapping]:
        """Load only units containing the requested candidate keys, in bounded batches."""
        result: dict[str, CandidateMapping] = {}
        unique = list(dict.fromkeys(keys))
        async with self.database.read() as tx:
            positions: list[tuple[str, str, int]] = []
            for offset in range(0, len(unique), 400):
                rows = (
                    await tx.connection.execute(
                        sa.select(
                            schema.snapshot_candidates.c.key,
                            schema.snapshot_candidates.c.unit_signature,
                            schema.snapshot_candidates.c.unit_index,
                        ).where(
                            schema.snapshot_candidates.c.snapshot_id == ref.id,
                            schema.snapshot_candidates.c.key.in_(unique[offset : offset + 400]),
                        )
                    )
                ).mappings()
                for row in rows:
                    positions.append((row["key"], row["unit_signature"], row["unit_index"]))
            signatures = list({signature for _, signature, _ in positions})
            units: dict[str, list[CandidateMapping]] = {}
            for offset in range(0, len(signatures), 400):
                rows = (
                    await tx.connection.execute(
                        sa.select(
                            schema.projection_units.c.signature,
                            schema.projection_units.c.candidates,
                        ).where(
                            schema.projection_units.c.signature.in_(
                                signatures[offset : offset + 400]
                            )
                        )
                    )
                ).mappings()
                for row in rows:
                    units[row["signature"]] = row["candidates"]
            for key, signature, index in positions:
                result[key] = units[signature][index]
        return result

    async def resolve(self, ref: str | SnapshotRef) -> SnapshotRow:
        row = await self._lookup(ref, include_mapping=True)
        if row is None:
            raise NousError(
                "wave.snapshot_not_found", "Wave snapshot does not exist", details={"ref": str(ref)}
            )
        async with self.database.read() as tx:
            members = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.projection_units.c.signature,
                            schema.projection_units.c.document_key,
                            schema.projection_units.c.document,
                            schema.projection_units.c.roots,
                            schema.projection_units.c.candidates,
                        )
                        .select_from(schema.snapshot_members.join(schema.projection_units))
                        .where(schema.snapshot_members.c.snapshot_id == row["id"])
                        .order_by(schema.snapshot_members.c.ordinal)
                    )
                )
                .mappings()
                .all()
            )
            positions = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.snapshot_candidates.c.unit_signature,
                            schema.snapshot_candidates.c.unit_index,
                            schema.snapshot_candidates.c.row_index,
                        )
                        .where(schema.snapshot_candidates.c.snapshot_id == row["id"])
                        .order_by(schema.snapshot_candidates.c.row_index)
                    )
                )
                .mappings()
                .all()
            )
            graph = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.projection_graphs.c.concepts,
                            schema.projection_graphs.c.associations,
                            schema.projection_graphs.c.support_groups,
                        ).where(schema.projection_graphs.c.signature == row["graph_signature"])
                    )
                )
                .mappings()
                .one()
            )
        documents = {member["document_key"]: member["document"] for member in members}
        units = {member["signature"]: member["candidates"] for member in members}
        roots = {key: refs for member in members for key, refs in member["roots"].items()}
        candidates = [
            units[position["unit_signature"]][position["unit_index"]] for position in positions
        ]
        value = dict(row)
        value["mapping"] = {
            **value.pop("layout"),
            "name": value["name"],
            "owner": value["owner"],
            "metadata": value["metadata"],
            "concepts": graph["concepts"],
            "associations": graph["associations"],
            "support_groups": graph["support_groups"],
            "roots": roots,
            "documents": documents,
            "candidates": candidates,
        }
        return cast(SnapshotRow, value)

    @asynccontextmanager
    async def pin(self, ref: str | SnapshotRef) -> AsyncGenerator[SnapshotRow]:
        """Keep a snapshot registered while its caller uses its mapping or files."""
        async with self._lifetime_lock:
            row = await self.resolve(ref)
            object_id = row["id"]
            self._borrowed[object_id] = self._borrowed.get(object_id, 0) + 1
        try:
            yield row
        finally:
            async with self._lifetime_lock:
                self._borrowed[object_id] -= 1
                if self._borrowed[object_id] == 0:
                    del self._borrowed[object_id]

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

                    asset, units, graph = await self.execution.blocking(
                        self._manifest_references, path
                    )
                    pending_assets.add(asset)
                    pending_units.update(units)
                    pending_graphs.add(graph)
            async with self.database.write() as tx:
                rows = (
                    (
                        await tx.connection.execute(
                            sa.select(
                                schema.snapshots.c.id,
                                schema.snapshots.c.identifier,
                                schema.snapshots.c.index_location,
                                schema.snapshots.c.collection_id,
                                schema.collections.c.current_snapshot_id,
                            )
                            .join(schema.collections)
                            .order_by(
                                schema.snapshots.c.created_at.desc(), schema.snapshots.c.id.desc()
                            )
                        )
                    )
                    .mappings()
                    .all()
                )
                by_id = {row["id"]: row for row in rows}
                for ref in retain:
                    if (
                        ref.instance_id != self.instance_id
                        or ref.id not in by_id
                        or by_id[ref.id]["identifier"] != ref.identifier
                    ):
                        raise NousError(
                            "wave.snapshot_not_found", "Retained snapshot does not exist"
                        )
                protected = (
                    set(self._borrowed) | set(self._prepared_counts) | {ref.id for ref in retain}
                )
                protected.update(row["current_snapshot_id"] for row in rows)
                protected.update(
                    (
                        await tx.connection.scalars(
                            sa.select(schema.snapshot_references.c.snapshot_id)
                        )
                    ).all()
                )
                per_collection: dict[str, int] = {}
                for row in rows:
                    count = per_collection.get(row["collection_id"], 0)
                    if count < keep_recent:
                        protected.add(row["id"])
                    per_collection[row["collection_id"]] = count + 1
                obsolete = [row for row in rows if row["id"] not in protected]
                ids = [row["id"] for row in obsolete]
                unused_units: set[str] = set()
                unused_graphs: set[str] = set()
                if ids:
                    for table in (
                        schema.snapshot_candidates,
                        schema.snapshot_members,
                        schema.snapshot_artifacts,
                    ):
                        await tx.connection.execute(
                            table.delete().where(table.c.snapshot_id.in_(ids))
                        )
                    await tx.connection.execute(
                        schema.snapshots.delete().where(schema.snapshots.c.id.in_(ids))
                    )
                    unused = ~sa.exists().where(
                        schema.snapshot_members.c.unit_signature
                        == schema.projection_units.c.signature
                    )
                    unused_units = set(
                        (
                            await tx.connection.scalars(
                                sa.select(schema.projection_units.c.signature).where(unused)
                            )
                        ).all()
                    )
                    if unused_units:
                        for table in (schema.projection_vectors, schema.projection_artifacts):
                            await tx.connection.execute(
                                table.delete().where(table.c.unit_signature.in_(unused_units))
                            )
                    await tx.connection.execute(schema.projection_units.delete().where(unused))
                    unused_graph = ~sa.exists().where(
                        schema.snapshots.c.graph_signature == schema.projection_graphs.c.signature
                    )
                    unused_graphs = set(
                        (
                            await tx.connection.scalars(
                                sa.select(schema.projection_graphs.c.signature).where(unused_graph)
                            )
                        ).all()
                    )
                    if unused_graphs:
                        for table in (
                            schema.projection_graph_vectors,
                            schema.projection_graph_artifacts,
                        ):
                            await tx.connection.execute(
                                table.delete().where(table.c.graph_signature.in_(unused_graphs))
                            )
                    await tx.connection.execute(
                        schema.projection_graphs.delete().where(unused_graph)
                    )
                    removed = [(row["id"], row["identifier"]) for row in obsolete]
                remaining_assets = {
                    row["index_location"] for row in rows if row["id"] not in ids
                } | pending_assets
                removed_assets = {row["index_location"] for row in obsolete} - remaining_assets
            if self._cached_snapshot and self._cached_snapshot[0] in {item[0] for item in removed}:
                self._cached_snapshot = None
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
