"""Collection publication, snapshot loading and source-aware Wave queries."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shutil
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, cast
from uuid import uuid4

import numpy as np
import numpy.typing as npt
import sqlalchemy as sa
from pydantic import JsonValue
from sqlalchemy.dialects.sqlite import insert

from nous.application.embedding import EmbeddingOperations
from nous.common.address import new_address
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection
from nous.content.store import ArtifactStore, ResultStore
from nous.media.preparation import MediaPreparer
from nous.models.embeddings import resolved_space
from nous.runtime.execution import ExecutionManager
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database
from nous.temporal.domains import TimeService
from nous.temporal.values import CognitiveTimes, Instant, Unknown, match_range

from . import schema
from .assets import ConceptMapping, FrozenInputs, SnapshotMapping, SnapshotRow, freeze_inputs
from .configuration import require_native_profiles
from .contracts import CandidateFilter, CollectionManifest, Concept, ContextFragment, WaveQuery

if TYPE_CHECKING:
    import nous_native


def _json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _signature(value: object) -> str:
    return hashlib.sha256(_json(value).encode("utf-8")).hexdigest()


class WaveOperations:
    """Own snapshots and their source mappings; Native owns rebuildable calculations."""

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

    def _native(self) -> nous_native.NativeEngine:
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
            self._engine = native.NativeEngine(_json({"threads": settings.threads}))
        return self._engine

    def close(self) -> None:
        if self._engine is not None:
            self._engine.close()
            self._engine = None
        self._cached_snapshot = None

    async def _compute[T](self, call: Callable[[], T], token: nous_native.CancellationToken) -> T:
        from nous_native import NativeError

        try:
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
                _json(frozen.candidate_metadata),
                frozen.candidate_texts,
                frozen.candidate_vectors,
                frozen.membership_indptr,
                frozen.membership_indices,
                frozen.curve_indptr,
                frozen.curve_indices,
                frozen.curve_positions,
                frozen.candidate_root_indptr,
                frozen.candidate_root_indices,
            ),
            native.Concepts(_json(frozen.concept_metadata), frozen.concept_vectors),
            native.SourceFacts(
                _json(frozen.facts_metadata),
                frozen.sequence_roots,
                frozen.sequence_indptr,
                frozen.sequence_indices,
                frozen.sequence_positions,
                frozen.sequence_ordered,
            ),
            native.Associations(
                _json(frozen.association_metadata),
                frozen.association_sources,
                frozen.association_targets,
                frozen.association_directed,
                frozen.association_strengths,
                frozen.association_confidences,
                frozen.association_polarities,
                frozen.association_root_indptr,
                frozen.association_root_indices,
            ),
        )

    async def build(self, manifest_path: Path, space: str | None = None) -> dict[str, object]:
        async with self.execution.operation("wave.build") as context:
            path = await self.execution.blocking(manifest_path.resolve)
            manifest_artifact = await self.artifacts.ingest(path)
            manifest = CollectionManifest.model_validate_json(
                await self.artifacts.read_text(manifest_artifact.ref)
            )
            await self.time.validate(document.times for document in manifest.documents)
            if space and manifest.space and space != manifest.space:
                raise NousError(
                    "wave.space_conflict", "Manifest and command select different spaces"
                )
            space_id = space or manifest.space
            definition = resolved_space(context.settings, space_id) if space_id else None
            wave = context.settings.typed_values().wave
            build_profile = {
                "native": wave.native.build,
                "associations": wave.associations.model_dump(mode="json"),
                "concept_template": "label-description-aliases-v1",
            }
            identifier = str(uuid4())
            identity = {
                "snapshot_id": identifier,
                "space_signature": _signature(
                    definition.model_dump(mode="json") if definition else None
                ),
                "build_config_signature": _signature(build_profile),
            }
            async with self._build_locks.setdefault(manifest.name, asyncio.Lock()):
                frozen = await freeze_inputs(
                    manifest,
                    path.parent,
                    space_id,
                    self.artifacts,
                    self.results,
                    self.embedding,
                    self.preparer,
                )
                engine = self._native()
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
                    snapshot = await self._compute(
                        lambda: engine.build(
                            *bundles,
                            _json(identity),
                            str(staging),
                            options_json=_json(build_profile),
                            cancel=token,
                        ),
                        token,
                    )
                    capabilities = list(snapshot.capabilities)
                    statistics = json.loads(snapshot.statistics_json)
                    # Windows requires readers to release staged files before the directory rename.
                    del snapshot
                    try:
                        await self.execution.blocking(os.replace, staging, final)
                    except asyncio.CancelledError:
                        # The worker may have completed the rename before cancellation surfaced.
                        published = final.is_dir() and not staging.exists()
                        raise
                    else:
                        published = True
                    # Reopen exactly what will be referenced before making it current.
                    reopened = await self._compute(
                        lambda: engine.open(str(final), _json(identity)), token
                    )
                    async with self.database.write() as tx:
                        await tx.connection.execute(
                            insert(schema.collections)
                            .values(id=str(uuid4()), name=manifest.name)
                            .on_conflict_do_nothing(index_elements=[schema.collections.c.name])
                        )
                        collection_id = await tx.connection.scalar(
                            sa.select(schema.collections.c.id).where(
                                schema.collections.c.name == manifest.name
                            )
                        )
                        if collection_id is None:
                            raise NousError(
                                "wave.collection_unavailable", "Cannot create collection"
                            )
                        address = ""
                        for _ in range(32):
                            address = new_address("wav")
                            inserted = await tx.connection.execute(
                                insert(schema.snapshots)
                                .values(
                                    id=identifier,
                                    address=address,
                                    collection_id=collection_id,
                                    manifest_artifact_id=manifest_artifact.ref.id,
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
                            raise NousError(
                                "wave.address_exhausted", "Cannot allocate a Wave address"
                            )
                        documents = cast(dict[str, dict[str, object]], frozen.mapping["documents"])
                        roots = frozen.mapping["roots"]
                        input_refs = {manifest_artifact.ref.id}
                        for document in documents.values():
                            input_refs.add(cast(dict[str, str], document["artifact"])["id"])
                        for refs in roots.values():
                            input_refs.update(ref["id"] for ref in refs)
                        for candidate in frozen.mapping["candidates"]:
                            input_refs.add(
                                ContentSelection.model_validate(candidate["content"]).artifact.id
                            )
                        for row in frozen.vector_mappings:
                            input_refs.add(
                                cast(dict[str, dict[str, str]], row["input"])["artifact"]["id"]
                            )
                        if input_refs:
                            await tx.connection.execute(
                                schema.snapshot_artifacts.insert(),
                                [
                                    {"snapshot_id": identifier, "artifact_id": ref}
                                    for ref in input_refs
                                ],
                            )
                        if frozen.vector_mappings:
                            await tx.connection.execute(
                                schema.snapshot_vectors.insert(),
                                [
                                    {"snapshot_id": identifier, **row}
                                    for row in frozen.vector_mappings
                                ],
                            )
                        await tx.connection.execute(
                            schema.collections.update()
                            .where(schema.collections.c.id == collection_id)
                            .values(current_snapshot_id=identifier)
                        )
                    self._cached_snapshot = (identifier, reopened)
                except BaseException:
                    if not published:
                        await self.execution.blocking(
                            self._remove_owned, staging, root, staging_name
                        )
                    raise
                return {
                    "snapshot": {
                        "instance_id": self.instance_id,
                        "id": identifier,
                        "address": address,
                    },
                    "collection": manifest.name,
                    "space": space_id,
                    "capabilities": capabilities,
                    "statistics": statistics,
                    "identity": identity,
                }

    async def _snapshot_row(self, ref: str) -> SnapshotRow:
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
            if row is None:
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
        return cast(SnapshotRow, dict(row))

    async def _open(
        self, row: SnapshotRow
    ) -> tuple[nous_native.NativeSnapshot, nous_native.CancellationToken]:
        engine = self._native()
        import nous_native as native

        token = native.CancellationToken()
        async with self._open_lock:
            if self._cached_snapshot is not None and self._cached_snapshot[0] == row["id"]:
                snapshot = self._cached_snapshot[1]
            else:
                path = self.paths.resolve_owned("index", row["index_location"])
                snapshot = await self._compute(
                    lambda: engine.open(str(path), _json(row["identity"])), token
                )
                self._cached_snapshot = (row["id"], snapshot)
        return snapshot, token

    async def inspect(self, ref: str) -> dict[str, object]:
        async with self.execution.operation("wave.inspect"):
            row = await self._snapshot_row(ref)
            snapshot, _ = await self._open(row)
            return {
                "snapshot": {
                    "instance_id": self.instance_id,
                    "id": row["id"],
                    "address": row["address"],
                },
                "collection": row["mapping"]["name"],
                "space": row["space_definition"],
                "identity": row["identity"],
                "capabilities": list(snapshot.capabilities),
                "statistics": json.loads(snapshot.statistics_json),
                "source_roots": len(row["mapping"]["roots"]),
                "created_at": row["created_at"],
            }

    async def read(
        self, ref: str, candidate_key: str, *, sources: bool = False
    ) -> dict[str, object]:
        async with self.execution.operation("wave.read"):
            row = await self._snapshot_row(ref)
            mapping = row["mapping"]
            candidate = next(
                (item for item in mapping["candidates"] if item["key"] == candidate_key), None
            )
            if candidate is None:
                raise NousError(
                    "wave.candidate_not_found",
                    "Candidate does not exist in this snapshot",
                    details={"candidate": candidate_key},
                )
            content = ContentSelection.model_validate(candidate["content"])
            result: dict[str, object] = {
                "snapshot": row["address"],
                "candidate": candidate_key,
                "content": content.model_dump(mode="json"),
                "origin_content": candidate["origin_content"],
                "text": await self.artifacts.read_selection(content),
                "metadata": candidate["metadata"],
            }
            if sources:
                result["sources"] = [
                    {
                        "evidence_root": root,
                        "artifacts": mapping["roots"][root],
                    }
                    for root in candidate["roots"]
                ]
            return result

    @staticmethod
    def _allowed_rows(mapping: SnapshotMapping, requested: CandidateFilter | None) -> list[int]:
        candidates = mapping["candidates"]
        if requested is None:
            return list(range(len(candidates)))
        available = {item["key"] for item in candidates}
        selected = (
            set(requested.candidate_keys) if requested.candidate_keys is not None else available
        )
        unknown = selected - available
        if unknown:
            raise NousError(
                "wave.candidate_not_found",
                "Candidate filter refers to unknown keys",
                details={"candidates": sorted(unknown)},
            )
        rows: list[int] = []
        for index, candidate in enumerate(candidates):
            if candidate["key"] not in selected:
                continue
            if requested.times:
                # All time conditions must match one actual source/declaration, not a mixture.
                root_keys = set(candidate["roots"])
                groups = [
                    document
                    for key, document in mapping["documents"].items()
                    if key == candidate["content_document"]
                    or root_keys.intersection(document["roots"])
                ]
                matches_group = False
                for group in groups:
                    times = (
                        CognitiveTimes.model_validate(group["times"]) if group["times"] else None
                    )
                    states: list[str] = []
                    for condition in requested.times:
                        value = getattr(times, condition.axis) if times is not None else None
                        if condition.axis == "recorded" and value is not None:
                            value = Instant(at=value)
                        states.append(match_range(value or Unknown(), condition.range))
                    if all(
                        state == "matches" or (state == "unknown" and requested.include_unknown)
                        for state in states
                    ):
                        matches_group = True
                        break
                if not matches_group:
                    continue
            rows.append(index)
        return rows

    async def _query_inputs(
        self, request: WaveQuery, row: SnapshotRow
    ) -> tuple[npt.NDArray[np.float32] | None, npt.NDArray[np.float32] | None, dict[str, object]]:
        import nous_native as native

        mode = request.mode or self.execution.snapshot.typed_values().wave.query.default_mode
        concepts = row["mapping"]["concepts"]
        by_key = {item["key"]: item for item in concepts}
        by_label: dict[str, list[ConceptMapping]] = {}
        for item in concepts:
            by_label.setdefault(item["label"], []).append(item)
            for alias in item["aliases"]:
                by_label.setdefault(alias, []).append(item)
        resolved_core: list[ConceptMapping] = []
        for key in request.core_concepts:
            if key in by_key:
                resolved_core.append(by_key[key])
                continue
            matches = list({item["key"]: item for item in by_label.get(key, [])}.values())
            if len(matches) != 1:
                raise NousError(
                    "wave.concept_ambiguous" if matches else "wave.concept_not_found",
                    "Core concept must identify one snapshot concept",
                    details={"concept": key, "matches": [item["key"] for item in matches]},
                )
            resolved_core.append(matches[0])
        if mode == "lexical" or row["space_id"] is None and mode == "hybrid":
            return (
                None,
                None,
                {
                    "query_origin": "text",
                    "context_used": False,
                    "core_keys": [concept["key"] for concept in resolved_core],
                    "core_labels": [concept["label"] for concept in resolved_core],
                },
            )
        if row["space_id"] is None:
            raise NousError("wave.capability_unavailable", "This snapshot has no vector space")
        definition = resolved_space(self.execution.snapshot, row["space_id"])
        if definition.model_dump(mode="json") != row["space_definition"]:
            raise NousError("wave.space_mismatch", "Snapshot and configured vector space differ")
        options = self.execution.snapshot.typed_values().wave.context
        query_selections: list[ContentSelection] = []
        raw_present = bool(request.text.strip())
        if raw_present:
            artifact = await self.artifacts.ingest(request.text, display_name="wave-query.txt")
            query_selections.append(ContentSelection(artifact=artifact.ref))
        fragments = (
            sorted(enumerate(request.context), key=lambda pair: (pair[1].sequence, pair[0]))
            if options.enabled
            else []
        )
        fragment_meta: list[dict[str, object]] = []
        for _, fragment in fragments:
            if fragment.content is not None:
                artifact = await self.artifacts.get(fragment.content.artifact)
                selected = ContentSelection(
                    artifact=artifact.ref, selection=fragment.content.selector.content_selection()
                )
            else:
                assert fragment.text is not None
                artifact = await self.artifacts.ingest(fragment.text, display_name=fragment.key)
                selected = ContentSelection(artifact=artifact.ref)
            query_selections.append(selected)
            fragment_meta.append(
                {
                    "key": fragment.key,
                    "role": fragment.role,
                    "sequence": fragment.sequence,
                    "weight": fragment.weight,
                }
            )
        concept_only = not raw_present and not fragment_meta
        if concept_only:
            for core_index, concept in enumerate(resolved_core):
                text = Concept.model_validate(concept).embedding_text()
                artifact = await self.artifacts.ingest(text, display_name=concept["key"])
                query_selections.append(ContentSelection(artifact=artifact.ref))
                fragment_meta.append(
                    {
                        "key": f"core:{core_index}",
                        "role": "concept",
                        "sequence": len(fragment_meta),
                        "weight": 1.0,
                    }
                )
            for cue_index, cue in enumerate(request.cues):
                artifact = await self.artifacts.ingest(cue.embedding_text(), display_name=cue.label)
                query_selections.append(ContentSelection(artifact=artifact.ref))
                fragment_meta.append(
                    {
                        "key": f"cue:{cue_index}",
                        "role": "concept",
                        "sequence": len(fragment_meta),
                        "weight": 1.0,
                    }
                )
        if not query_selections:
            raise NousError(
                "wave.empty_query", "Provide query text, context, a core concept or a cue"
            )
        query_batch = await self.embedding.ensure(query_selections, row["space_id"], "query")
        vectors = await self.embedding.read(query_batch)
        raw = vectors[0] if raw_present else None
        context_vectors = vectors[1:] if raw_present else vectors
        if len(context_vectors):
            aggregate_options: dict[str, object] = {
                "window": options.window,
                "decay": options.decay,
                "context_mix": options.mix,
                "role_weights": options.role_weights,
                "default_role_weight": options.default_role_weight,
            }
            if concept_only:
                aggregate_options.update(
                    {
                        "window": len(context_vectors),
                        "decay": 1.0,
                        "context_mix": 1.0,
                        "role_weights": {"concept": 1.0},
                    }
                )
            aggregated = await self.execution.blocking(
                native.aggregate_context,
                raw,
                context_vectors,
                _json(fragment_meta),
                _json(aggregate_options),
            )
            effective = aggregated.vector
            summary: dict[str, object] = json.loads(aggregated.diagnostics_json)
            summary.pop("effective_query", None)
            if concept_only:
                summary["query_origin"] = "concepts"
        else:
            effective = raw
            summary = {"query_origin": "text", "context_used": False}
        ghosts: npt.NDArray[np.float32] | None = None
        if request.cues:
            selections: list[ContentSelection] = []
            for cue in request.cues:
                artifact = await self.artifacts.ingest(cue.embedding_text(), display_name=cue.label)
                selections.append(ContentSelection(artifact=artifact.ref))
            ghost_batch = await self.embedding.ensure(selections, row["space_id"], "document")
            ghosts = await self.embedding.read(ghost_batch)
        summary["core_keys"] = [concept["key"] for concept in resolved_core]
        summary["core_labels"] = [concept["label"] for concept in resolved_core]
        return effective, ghosts, summary

    async def query(self, request: WaveQuery) -> dict[str, object]:
        async with self.execution.operation("wave.query") as context:
            await self.time.validate(fragment.time for fragment in request.context)
            if request.candidate_filter is not None:
                await self.time.validate(
                    condition.range for condition in request.candidate_filter.times
                )
            row = await self._snapshot_row(request.snapshot)
            snapshot, token = await self._open(row)
            wave = context.settings.typed_values().wave
            mode = request.mode or wave.query.default_mode
            mapping = row["mapping"]
            allowed = self._allowed_rows(mapping, request.candidate_filter)
            vector, ghosts, query_summary = await self._query_inputs(request, row)
            concepts = mapping["concepts"]
            keys = {item["key"]: index for index, item in enumerate(concepts)}
            core_rows = [keys[key] for key in cast(list[str], query_summary.get("core_keys", []))]
            candidates = mapping["candidates"]
            candidate_rows = {candidate["key"]: index for index, candidate in enumerate(candidates)}
            stages: list[dict[str, object]] = []
            for stage in request.thinking_stages:
                unknown = set(stage.candidates) - candidate_rows.keys()
                if unknown:
                    raise NousError(
                        "wave.candidate_not_found",
                        "Thinking stage refers to unknown candidates",
                        details={"candidates": sorted(unknown)},
                    )
                stages.append(
                    {
                        "name": stage.name,
                        "candidate_rows": [
                            candidate_rows[key]
                            for key in stage.candidates
                            if candidate_rows[key] in allowed
                        ],
                        "top_k": stage.top_k,
                    }
                )
            controls = cast(dict[str, JsonValue], wave.native.query["controls"])
            search_options = {
                **{name: value for name, value in wave.native.query.items() if name != "controls"},
                "candidate_budget": controls["candidate_budget"],
                "thinking_min_closure": controls["thinking_min_closure"],
                "top_k": request.top_k or wave.query.top_k,
                "include_trace": request.include_trace,
                "core_rows": core_rows,
                "ghosts": [{"name": cue.label, "is_core": cue.is_core} for cue in request.cues],
                "thinking_stages": stages,
                "dedup_threshold": wave.dedup.threshold if wave.dedup.semantic_enabled else None,
                "rrf_k": wave.fusion.rrf_k,
                "include_counterevidence": (
                    request.include_counterevidence
                    if request.include_counterevidence is not None
                    else wave.query.include_counterevidence
                ),
            }
            query_text = " ".join([request.text, *(cue.label for cue in request.cues)]).strip()
            if not query_text and mode == "lexical":
                query_text = " ".join(cast(list[str], query_summary.get("core_labels", [])))
            if mode == "lexical" and not query_text:
                raise NousError(
                    "wave.empty_query", "Lexical retrieval requires query text or cue labels"
                )
            result = await self._compute(
                lambda: snapshot.search(
                    mode=mode,
                    text=query_text,
                    vector=vector,
                    allowed_rows=np.asarray(allowed, dtype=np.uint64),
                    options_json=_json(search_options),
                    ghost_vectors=ghosts,
                    cancel=token,
                ),
                token,
            )
            values = json.loads(result.result_json)
            hits: list[dict[str, object]] = []
            for hit in values["hits"]:
                candidate = candidates[cast(int, hit["row"])]
                selection = ContentSelection.model_validate(candidate["content"])
                text = await self.artifacts.read_selection(selection)
                counterevidence: list[dict[str, object]] = []
                for related_row in hit.get("counterevidence_rows", []):
                    related = candidates[cast(int, related_row)]
                    current_concepts = set(candidate["concepts"])
                    related_concepts = set(related["concepts"])
                    for association in mapping["associations"]:
                        if association["polarity"] != "negative":
                            continue
                        forward = (
                            association["source"] in current_concepts
                            and association["target"] in related_concepts
                        )
                        reverse = (
                            not association["directed"]
                            and association["target"] in current_concepts
                            and association["source"] in related_concepts
                        )
                        if forward or reverse:
                            counterevidence.append(
                                {
                                    "candidate": related["key"],
                                    "content": related["content"],
                                    "relation": association["relation"],
                                    "source": association["source"],
                                    "target": association["target"],
                                    "evidence": [
                                        {"root": root, "artifacts": mapping["roots"][root]}
                                        for root in association["roots"]
                                    ],
                                }
                            )
                converted: dict[str, object] = {
                    **hit,
                    "candidate": candidate["key"],
                    "candidate_ref": {"snapshot": row["address"], "key": candidate["key"]},
                    "content": candidate["content"],
                    "origin_content": candidate["origin_content"],
                    "snippet": text[:500],
                    "concepts": [concepts[index]["key"] for index in hit.get("concept_rows", [])],
                    "sources": candidate["roots"],
                    "folded_refs": [
                        {
                            "candidate": candidates[item["row"]]["key"],
                            "similarity": item["similarity"],
                        }
                        for item in hit.get("folded_rows", [])
                    ],
                    "counterevidence_refs": [
                        candidates[index]["key"] for index in hit.get("counterevidence_rows", [])
                    ],
                    "counterevidence": counterevidence,
                }
                if not request.include_trace:
                    converted.pop("trace", None)
                for internal in (
                    "row",
                    "concept_rows",
                    "evidence_roots",
                    "folded_rows",
                    "counterevidence_rows",
                ):
                    converted.pop(internal, None)
                hits.append(converted)
            thinking = values.get("thinking")
            if isinstance(thinking, dict):
                thinking = cast(dict[str, object], thinking)
                for stage_value in cast(list[dict[str, object]], thinking.get("stages", [])):
                    for item in cast(list[dict[str, object]], stage_value.get("results", [])):
                        item["candidate"] = candidates[cast(int, item.pop("row"))]["key"]
                        item["parents"] = [
                            candidates[index]["key"]
                            for index in cast(list[int], item.get("parents", []))
                        ]
                        route = item.get("route")
                        if isinstance(route, dict):
                            route = cast(dict[str, object], route)
                            for field, output_field in (
                                ("sourceId", "source"),
                                ("targetId", "target"),
                                ("previousId", "previous"),
                            ):
                                concept_id = route.pop(field, None)
                                if concept_id is None:
                                    route[output_field] = None
                                    continue
                                if (
                                    not isinstance(concept_id, int)
                                    or isinstance(concept_id, bool)
                                    or not 1 <= concept_id <= len(concepts)
                                ):
                                    raise NousError(
                                        "wave.invalid_native_result",
                                        "Thinking route references an unknown concept",
                                    )
                                route[output_field] = concepts[concept_id - 1]["key"]
            output: dict[str, object] = {
                "snapshot": {
                    "instance_id": self.instance_id,
                    "id": row["id"],
                    "address": row["address"],
                },
                "mode": values["mode"],
                "score_kind": values["score_kind"],
                "used_lanes": values["used_lanes"],
                "skipped_lanes": values["skipped_lanes"],
                "hits": hits,
                "context_summary": query_summary,
                "cue_summary": [
                    {"label": cue.label, "is_core": cue.is_core} for cue in request.cues
                ],
                "algorithm_versions": values["algorithm_versions"],
                "scope": {
                    "source_roots": len(mapping["roots"]),
                    "candidate_filter": request.candidate_filter is not None,
                },
                "coverage": values["coverage"],
                "thinking": thinking,
            }
            if request.include_trace:
                output["diagnostics"] = values["diagnostics"]
            return output

    async def neighborhood(self, ref: str, concepts: Sequence[str]) -> dict[str, object]:
        async with self.execution.operation("wave.neighborhood"):
            row = await self._snapshot_row(ref)
            snapshot, token = await self._open(row)
            all_concepts = row["mapping"]["concepts"]
            rows = {item["key"]: index for index, item in enumerate(all_concepts)}
            unknown = set(concepts) - rows.keys()
            if unknown:
                raise NousError(
                    "wave.concept_not_found",
                    "Neighborhood refers to unknown concept keys",
                    details={"concepts": sorted(unknown)},
                )
            result = await self._compute(
                lambda: snapshot.neighborhood(
                    np.asarray([rows[key] for key in concepts], dtype=np.uint64), cancel=token
                ),
                token,
            )
            neighborhood = json.loads(result)
            for edge in neighborhood.get("edges", []):
                edge["source"] = all_concepts[edge["source"]]["key"]
                edge["target"] = all_concepts[edge["target"]]["key"]
            for node in neighborhood.get("nodes", []):
                node["concept"] = all_concepts[node.pop("row")]["key"]
                node["roots"] = [
                    row["mapping"]["root_order"][index] for index in node.pop("root_rows", [])
                ]
                node["candidates"] = [
                    row["mapping"]["candidates"][index]["key"]
                    for index in node.pop("candidate_rows", [])
                ]
            for association in neighborhood.get("associations", []):
                association["source"] = all_concepts[association["source"]]["key"]
                association["target"] = all_concepts[association["target"]]["key"]
                association["evidence_roots"] = [
                    row["mapping"]["root_order"][index] for index in association["evidence_roots"]
                ]
            return {
                "snapshot": row["address"],
                "concepts": list(concepts),
                "neighborhood": neighborhood,
            }

    async def segment(self, ref: str, fragments: Sequence[ContextFragment]) -> dict[str, object]:
        async with self.execution.operation("wave.segment") as context:
            await self.time.validate(fragment.time for fragment in fragments)
            row = await self._snapshot_row(ref)
            if row["space_id"] is None:
                raise NousError("wave.capability_unavailable", "This snapshot has no vector space")
            definition = resolved_space(context.settings, row["space_id"])
            if definition.model_dump(mode="json") != row["space_definition"]:
                raise NousError(
                    "wave.space_mismatch", "Snapshot and configured vector space differ"
                )
            self._native()
            import nous_native as native

            ordered = sorted(enumerate(fragments), key=lambda pair: (pair[1].sequence, pair[0]))
            selections: list[ContentSelection] = []
            for _, fragment in ordered:
                if fragment.content is not None:
                    artifact = await self.artifacts.get(fragment.content.artifact)
                    selections.append(
                        ContentSelection(
                            artifact=artifact.ref,
                            selection=fragment.content.selector.content_selection(),
                        )
                    )
                else:
                    assert fragment.text is not None
                    artifact = await self.artifacts.ingest(fragment.text, display_name=fragment.key)
                    selections.append(ContentSelection(artifact=artifact.ref))
            if not selections:
                return {"snapshot": row["address"], "segments": []}
            batch = await self.embedding.ensure(selections, row["space_id"], "query")
            vectors = await self.embedding.read(batch)
            result = await self.execution.blocking(
                native.segment,
                vectors,
                context.settings.typed_values().wave.context.segment_threshold,
            )
            ranges = json.loads(result.ranges_json)
            keys = [fragment.key for _, fragment in ordered]
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
