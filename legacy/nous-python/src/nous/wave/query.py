"""Query signals, candidate ranges and projection back to exact content references."""

from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, cast

import numpy as np
import numpy.typing as npt
from pydantic import JsonValue

from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, ExternalInput
from nous.models.embeddings import resolved_space
from nous.models.vector_contracts import VectorRef
from nous.runtime.execution import ExecutionManager
from nous.temporal.domains import TimeService
from nous.temporal.values import CognitiveTimes, Instant, Unknown, match_range

from .contracts import (
    CandidateFilter,
    CandidatePool,
    Concept,
    ContextFragment,
    ThinkingStage,
    WaveNeighborhoodResult,
    WaveQuery,
    WaveReadResult,
)
from .mapping import (
    ConceptMapping,
    SnapshotMapping,
    SnapshotRow,
)
from .serialization import encode_json

if TYPE_CHECKING:
    import nous_native


@dataclass(frozen=True)
class BoundQuery:
    """WaveQuery after external inputs have been read and validated."""

    vector: VectorRef | None
    candidate_pool: CandidatePool | None
    context: tuple[ContextFragment, ...]
    candidate_filter: CandidateFilter | None
    thinking_stages: tuple[ThinkingStage, ...]


def require_bound_query(request: WaveQuery) -> BoundQuery:
    """Reject a query that still carries unread external inputs."""
    vector = request.vector
    candidate_pool = request.candidate_pool
    context = request.context
    candidate_filter = request.candidate_filter
    thinking_stages = request.thinking_stages
    if (
        isinstance(vector, ExternalInput)
        or isinstance(candidate_pool, ExternalInput)
        or isinstance(context, ExternalInput)
        or isinstance(candidate_filter, ExternalInput)
        or isinstance(thinking_stages, ExternalInput)
    ):
        raise NousError(
            "input.missing_binding", "External query inputs require an application call"
        )
    return BoundQuery(vector, candidate_pool, context, candidate_filter, thinking_stages)


@dataclass(frozen=True)
class QueryPlan:
    mode: str
    text: str
    vector: npt.NDArray[np.float32] | None
    ghosts: npt.NDArray[np.float32] | None
    allowed: npt.NDArray[np.uint64]
    options: Mapping[str, object]
    pool: nous_native.CandidatePool | None
    summary: dict[str, object]


class WaveQueries:
    """Resolve semantic inputs and public references; Native owns every numerical score."""

    def __init__(
        self,
        execution: ExecutionManager,
        artifacts: ArtifactStore,
        embedding: EmbeddingOperations,
        time: TimeService,
        instance_id: str,
    ) -> None:
        self.execution, self.artifacts, self.embedding, self.time, self.instance_id = (
            execution,
            artifacts,
            embedding,
            time,
            instance_id,
        )

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

        bound = require_bound_query(request)
        mode = request.mode or self.execution.snapshot.values.wave.query.default_mode
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
            if identifier_type(key) == "con":
                versions = [item for item in concepts if item["key"].startswith(key + "@")]
                if versions:
                    resolved_core.extend(versions)
                    continue
            matches = list({item["key"]: item for item in by_label.get(key, [])}.values())
            if len(matches) != 1:
                raise NousError(
                    "wave.concept_ambiguous" if matches else "wave.concept_not_found",
                    "Core concept must identify one snapshot concept",
                    details={"concept": key, "matches": [item["key"] for item in matches]},
                )
            resolved_core.append(matches[0])
        if mode == "lexical" and bound.vector is not None:
            raise NousError("wave.invalid_input", "Lexical mode does not consume query vectors")
        if mode == "lexical" or row["space_id"] is None and mode == "hybrid":
            return (
                None,
                None,
                {
                    "query_origin": "text",
                    "context_used": False,
                    "selected_context": [],
                    "omitted_context_count": len(bound.context),
                    "core_keys": [concept["key"] for concept in resolved_core],
                    "core_labels": [concept["label"] for concept in resolved_core],
                },
            )
        if row["space_id"] is None:
            raise NousError("wave.capability_unavailable", "This snapshot has no vector space")
        definition = resolved_space(self.execution.snapshot, row["space_id"])
        if definition.model_dump(mode="json") != row["space_definition"]:
            raise NousError("wave.space_mismatch", "Snapshot and configured vector space differ")
        options = self.execution.snapshot.values.wave.context
        query_selections: list[str | ContentSelection] = []
        raw_vector = None
        if bound.vector is not None:
            if (
                bound.vector.space.model_dump(mode="json") != row["space_definition"]
                or bound.vector.purpose != "query"
            ):
                raise NousError(
                    "wave.space_mismatch",
                    "Query vector requires this snapshot space and query purpose",
                )
            raw_vector = await self.embedding.read_vector(bound.vector)
        raw_present = raw_vector is not None or bool(request.text.strip())
        encoded_raw = raw_vector is None and bool(request.text.strip())
        if encoded_raw:
            query_selections.append(request.text)
        fragments = (
            sorted(enumerate(bound.context), key=lambda pair: (pair[1].sequence, pair[0]))
            if options.enabled
            else []
        )
        fragment_meta: list[dict[str, object]] = []
        resolved_fragments: dict[str, dict[str, object]] = {}
        for _, fragment in fragments:
            if fragment.content is not None:
                artifact = await self.artifacts.get(fragment.content.artifact)
                selected = ContentSelection(
                    artifact=artifact.ref, selection=fragment.content.selector
                )
            else:
                assert fragment.text is not None
                selected = fragment.text
            query_selections.append(selected)
            resolved_fragments[fragment.key] = {
                "key": fragment.key,
                "role": fragment.role,
                "sequence": fragment.sequence,
                "weight": fragment.weight,
                "time": fragment.time.model_dump(mode="json") if fragment.time else None,
                "content": selected.model_dump(mode="json")
                if isinstance(selected, ContentSelection)
                else None,
                "text": selected if isinstance(selected, str) else None,
            }
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
                query_selections.append(text)
                fragment_meta.append(
                    {
                        "key": f"core:{core_index}",
                        "role": "concept",
                        "sequence": len(fragment_meta),
                        "weight": 1.0,
                    }
                )
            for cue_index, cue in enumerate(request.cues):
                query_selections.append(cue.embedding_text())
                fragment_meta.append(
                    {
                        "key": f"cue:{cue_index}",
                        "role": "concept",
                        "sequence": len(fragment_meta),
                        "weight": 1.0,
                    }
                )
        if not query_selections and raw_vector is None:
            raise NousError(
                "wave.empty_query", "Provide query text, context, a core concept or a cue"
            )
        vectors = (
            await self.embedding.encode_query(query_selections, row["space_id"])
            if query_selections
            else np.empty((0, len(raw_vector) if raw_vector is not None else 0), dtype=np.float32)
        )
        raw = raw_vector if raw_vector is not None else (vectors[0] if encoded_raw else None)
        context_vectors = vectors[1:] if encoded_raw else vectors
        if len(context_vectors):
            aggregate_options: dict[str, object] = {
                "window": options.window,
                "decay": options.decay,
                "context_mix": options.mix,
                "role_weights": dict(options.role_weights),
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
                encode_json(fragment_meta),
                encode_json(aggregate_options),
            )
            effective = aggregated.vector
            summary: dict[str, object] = json.loads(aggregated.diagnostics_json)
            summary.pop("effective_query", None)
            summary["context_options"] = aggregate_options
            if concept_only:
                summary["query_origin"] = "concepts"
        else:
            effective = raw
            summary = {"query_origin": "text", "context_used": False}
        native_selected = cast(list[dict[str, object]], summary.pop("selected_context", []))
        selected_context = [
            {
                **resolved_fragments[cast(str, part["key"])],
                "decayed_weight": part["decayed_weight"],
                "role_weight": part["role_weight"],
            }
            for part in native_selected
            if part["key"] in resolved_fragments
        ]
        summary["selected_context"] = selected_context
        summary["omitted_context_count"] = len(bound.context) - len(selected_context)
        ghosts: npt.NDArray[np.float32] | None = None
        if request.cues:
            ghosts = await self.embedding.encode_query(
                tuple(cue.embedding_text() for cue in request.cues), row["space_id"]
            )
        if bound.vector is not None:
            summary["query_origin"] = "vector+context" if len(context_vectors) else "vector"
            summary["vector_ref"] = {
                "artifact": bound.vector.artifact.identifier,
                "row_index": bound.vector.row_index,
            }
        summary["core_keys"] = [concept["key"] for concept in resolved_core]
        summary["core_labels"] = [concept["label"] for concept in resolved_core]
        return effective, ghosts, summary

    async def prepare(self, request: WaveQuery, row: SnapshotRow) -> QueryPlan:
        bound = require_bound_query(request)
        await self.time.validate(fragment.time for fragment in bound.context)
        if bound.candidate_filter is not None:
            await self.time.validate(condition.range for condition in bound.candidate_filter.times)
        wave = self.execution.snapshot.values.wave
        mode = request.mode or wave.query.default_mode
        query_settings = wave.native.model_dump(mode="json")["query"]
        readout = cast(dict[str, JsonValue], query_settings["readout"])
        maximum = cast(int, readout["maxUnionCandidates"])
        if request.grouping == "document" and (request.top_k or wave.query.top_k) > maximum:
            raise NousError(
                "wave.invalid_input",
                "Requested document count exceeds max_union_candidates",
                details={
                    "top_k": request.top_k or wave.query.top_k,
                    "max_union_candidates": maximum,
                },
            )
        mapping = row["mapping"]
        allowed = self._allowed_rows(mapping, bound.candidate_filter)
        vector, ghosts, query_summary = await self._query_inputs(request, row)
        concepts = mapping["concepts"]
        keys = {item["key"]: index for index, item in enumerate(concepts)}
        core_rows = [keys[key] for key in cast(list[str], query_summary.get("core_keys", []))]
        candidates = mapping["candidates"]
        candidate_rows = {candidate["key"]: index for index, candidate in enumerate(candidates)}
        stages: list[dict[str, object]] = []
        for stage in bound.thinking_stages:
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
        controls = cast(dict[str, JsonValue], query_settings["controls"])
        search_options = {
            **{name: value for name, value in query_settings.items() if name != "controls"},
            "query_space_signature": row["identity"]["space_signature"],
            "thinking_min_closure": controls["thinking_min_closure"],
            "top_k": request.top_k or wave.query.top_k,
            "grouping": request.grouping,
            "passages_per_group": request.passages_per_group,
            "group_scan_limit": wave.query.group_scan_limit,
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
        native_pool = None
        if bound.candidate_pool is not None:
            pool = bound.candidate_pool
            if pool.snapshot not in {row["identifier"], row["id"]}:
                raise NousError(
                    "wave.candidate_pool_mismatch", "Candidate pool belongs to another snapshot"
                )
            unknown = {entry.candidate for entry in pool.entries} - candidate_rows.keys()
            if unknown:
                raise NousError(
                    "wave.candidate_not_found",
                    "Candidate pool contains unknown keys",
                    details={"candidates": sorted(unknown)},
                )
            import nous_native as native

            native_pool = native.CandidatePool(
                encode_json(
                    {
                        "identity": row["identity"],
                        "rank_scope": pool.rank_scope,
                        "entries": [
                            {
                                "row": candidate_rows[entry.candidate],
                                "lanes": {
                                    name: lane.model_dump(mode="json")
                                    for name, lane in entry.lanes.items()
                                },
                            }
                            for entry in pool.entries
                        ],
                    }
                )
            )
        query_summary["effective_query_options"] = {
            **search_options,
            "text": query_text,
            "context": query_summary.get("context_options"),
            "query_origin": query_summary.get("query_origin"),
        }
        query_summary["grouping"] = {
            "by": request.grouping,
            "top_k": request.top_k or wave.query.top_k,
            "passages_per_group": request.passages_per_group,
            "lane_limit_unit": "groups" if request.grouping == "document" else "candidates",
            "group_scan_limit": wave.query.group_scan_limit,
        }
        return QueryPlan(
            mode,
            query_text,
            vector,
            ghosts,
            np.asarray(allowed, dtype=np.uint64),
            search_options,
            native_pool,
            query_summary,
        )

    async def read(
        self, row: SnapshotRow, candidate_key: str, *, sources: bool = False
    ) -> WaveReadResult:
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
        return WaveReadResult.model_validate(
            {
                "snapshot": {
                    "instance_id": self.instance_id,
                    "id": row["id"],
                    "identifier": row["identifier"],
                },
                "candidate": {"snapshot": row["identifier"], "key": candidate_key},
                "content": content,
                "origin_contents": candidate["origin_contents"],
                "text": await self.artifacts.read_selection(content),
                "group": row["mapping"]["groups"][candidate["group_key"]],
                "concepts": candidate["concepts"],
                "paths": candidate["paths"],
                "sources": [
                    {"evidence_root": root, "artifacts": mapping["roots"][root]}
                    for root in candidate["roots"]
                ]
                if sources
                else None,
            }
        )

    @staticmethod
    def neighborhood_rows(row: SnapshotRow, concepts: Sequence[str]) -> npt.NDArray[np.uint64]:
        all_concepts = row["mapping"]["concepts"]
        rows = {item["key"]: index for index, item in enumerate(all_concepts)}
        unknown = set(concepts) - rows.keys()
        if unknown:
            raise NousError(
                "wave.concept_not_found",
                "Neighborhood refers to unknown concept keys",
                details={"concepts": sorted(unknown)},
            )
        return np.asarray([rows[key] for key in concepts], dtype=np.uint64)

    def neighborhood(
        self, row: SnapshotRow, concepts: Sequence[str], result: str
    ) -> WaveNeighborhoodResult:
        all_concepts = row["mapping"]["concepts"]
        neighborhood = json.loads(result)
        for edge in neighborhood.get("edges", []):
            edge["source"] = all_concepts[edge["source"]]["key"]
            edge["target"] = all_concepts[edge["target"]]["key"]
        for node in neighborhood.get("nodes", []):
            node["concept"] = all_concepts[node.pop("row")]["key"]
            node["roots"] = [
                row["mapping"]["root_order"][index] for index in node.pop("root_rows", [])
            ]
            node["source_witness_groups"] = [
                [row["mapping"]["root_order"][index] for index in group]
                for group in node.get("source_witness_groups", [])
            ]
            node["candidates"] = [
                row["mapping"]["candidates"][index]["key"]
                for index in node.pop("candidate_rows", [])
            ]
        for association in neighborhood.get("associations", []):
            association["source"] = all_concepts[association["source"]]["key"]
            association["target"] = all_concepts[association["target"]]["key"]
            group = association.get("support_group")
            if isinstance(group, dict):
                group = cast(dict[str, object], group)
                association["support_group"] = next(
                    (
                        item
                        for item in row["mapping"]["support_groups"]
                        if item["key"] == group.get("key")
                    ),
                    group,
                )
            association["evidence_roots"] = [
                row["mapping"]["root_order"][index] for index in association["evidence_roots"]
            ]
        return WaveNeighborhoodResult.model_validate(
            {
                "snapshot": {
                    "instance_id": self.instance_id,
                    "id": row["id"],
                    "identifier": row["identifier"],
                },
                "concepts": concepts,
                "neighborhood": neighborhood,
            }
        )

    async def segment_vectors(
        self, row: SnapshotRow, fragments: Sequence[ContextFragment]
    ) -> tuple[npt.NDArray[np.float32], list[str]]:
        await self.time.validate(fragment.time for fragment in fragments)
        if row["space_id"] is None:
            raise NousError("wave.capability_unavailable", "This snapshot has no vector space")
        definition = resolved_space(self.execution.snapshot, row["space_id"])
        if definition.model_dump(mode="json") != row["space_definition"]:
            raise NousError("wave.space_mismatch", "Snapshot and configured vector space differ")

        ordered = sorted(enumerate(fragments), key=lambda pair: (pair[1].sequence, pair[0]))
        selections: list[str | ContentSelection] = []
        for _, fragment in ordered:
            if fragment.content is not None:
                artifact = await self.artifacts.get(fragment.content.artifact)
                selections.append(
                    ContentSelection(
                        artifact=artifact.ref,
                        selection=fragment.content.selector,
                    )
                )
            else:
                assert fragment.text is not None
                selections.append(fragment.text)
        if not selections:
            return np.empty((0, definition.dimensions), dtype=np.float32), []
        vectors = await self.embedding.encode_query(selections, row["space_id"])
        return vectors, [fragment.key for _, fragment in ordered]
