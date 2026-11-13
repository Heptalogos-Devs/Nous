"""Query signals, candidate ranges and projection back to exact content references."""

from __future__ import annotations

import hashlib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, cast

import numpy as np
import numpy.typing as npt
from pydantic import BaseModel, ConfigDict, JsonValue

from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, ExternalInput
from nous.models.embeddings import resolved_space
from nous.models.vector_contracts import VectorRef
from nous.runtime.execution import ExecutionManager
from nous.temporal.domains import TimeService
from nous.temporal.values import (
    CognitiveTimes,
    Instant,
    Interval,
    Known,
    TimePoint,
)

from .contracts import (
    CandidateFilter,
    CandidateSet,
    Concept,
    ContextFragment,
    DeliveryBudget,
    GroupDescriptor,
    ProgramRunFacts,
    ThinkingStage,
    WaveExpression,
    WaveQuery,
)
from .mapping import (
    ConceptMapping,
    SnapshotRow,
)
from .program import CompiledProgram, EvaluationParameters, program_run_facts, resolve_program
from .scope import allowed_rows
from .serialization import encode_json

if TYPE_CHECKING:
    import nous_native


@dataclass(frozen=True)
class BoundQuery:
    """WaveQuery after external inputs have been read and validated."""

    vector: VectorRef | None
    candidate_set: CandidateSet | None
    context: tuple[ContextFragment, ...]
    candidate_filter: CandidateFilter | None
    thinking_stages: tuple[ThinkingStage, ...]


class _ContextAllocation(BaseModel):
    model_config = ConfigDict(extra="forbid")
    row: int
    key: str
    role: str
    stage: str
    parent_key: str
    contribution: float
    decayed_weight: float
    role_weight: float


class _ContextDiagnostics(BaseModel):
    model_config = ConfigDict(extra="forbid")
    selected_context: tuple[_ContextAllocation, ...] = ()
    roles: tuple[dict[str, JsonValue], ...] = ()
    query_origin: str | None = None
    effective_query: tuple[float, ...] | None = None
    context_mix: float | None = None


def require_bound_query(request: WaveQuery) -> BoundQuery:
    """Reject a query that still carries unread external inputs."""
    vector = request.vector
    candidate_set = request.candidate_set
    context = request.context
    candidate_filter = request.candidate_filter
    thinking_stages = request.thinking_stages
    if (
        isinstance(vector, ExternalInput)
        or isinstance(candidate_set, ExternalInput)
        or isinstance(context, ExternalInput)
        or isinstance(candidate_filter, ExternalInput)
        or isinstance(thinking_stages, ExternalInput)
    ):
        raise NousError(
            "input.missing_binding", "External query inputs require an application call"
        )
    return BoundQuery(vector, candidate_set, context, candidate_filter, thinking_stages)


@dataclass(frozen=True)
class QueryPlan:
    program: CompiledProgram
    text: str
    raw_vector: npt.NDArray[np.float32] | None
    contextual_vector: npt.NDArray[np.float32] | None
    ghosts: npt.NDArray[np.float32] | None
    allowed: npt.NDArray[np.uint64]
    options: Mapping[str, object]
    candidate_set: nous_native.CandidateSet | None
    summary: dict[str, object]
    program_run: ProgramRunFacts
    expression: dict[str, object] | None = None
    branch_raw_vectors: npt.NDArray[np.float32] | None = None
    branch_contextual_vectors: npt.NDArray[np.float32] | None = None


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

    async def _query_inputs(
        self,
        request: WaveQuery,
        row: SnapshotRow,
        program: CompiledProgram,
        *,
        vectors_required: bool | None = None,
        context_cache: dict[tuple[str, ...], npt.NDArray[np.float32]] | None = None,
        include_ghosts: bool = True,
    ) -> tuple[
        npt.NDArray[np.float32] | None,
        npt.NDArray[np.float32] | None,
        npt.NDArray[np.float32] | None,
        dict[str, object],
    ]:
        import nous_native as native

        bound = require_bound_query(request)
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
        needs_vectors = (
            "vectors" in program.requirements if vectors_required is None else vectors_required
        )
        if not needs_vectors and bound.vector is not None:
            raise NousError("wave.invalid_input", "This program does not consume query vectors")
        space_id = row["space_id"]
        if needs_vectors:
            if space_id is None:
                raise NousError("wave.capability_unavailable", "This snapshot has no vector space")
            definition = resolved_space(self.execution.snapshot, space_id)
            if definition.model_dump(mode="json") != row["space_definition"]:
                raise NousError(
                    "wave.space_mismatch", "Snapshot and configured vector space differ"
                )
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
        active_stages: set[str] = set()
        uses_contextual_signal = any(
            ref.node == "query" and ref.port == "contextual_signal"
            for node in program.nodes
            for value in node.inputs.values()
            for ref in (value if isinstance(value, tuple) else (value,))
        )
        if (
            any(node.operator == "context.recall" for node in program.nodes)
            or uses_contextual_signal
        ):
            active_stages.add("retrieval")
        if any(
            node.operator == "passages.locate"
            and isinstance(signals := node.parameters.get("signals", ["lexical", "semantic"]), list)
            and "lexical" in signals
            for node in program.nodes
        ):
            active_stages.add("localization")
        if any(node.operator == "passages.evaluate" for node in program.nodes):
            active_stages.add("evaluation")
        fragments = (
            sorted(
                (
                    (index, item)
                    for index, item in enumerate(bound.context)
                    if item.stage in active_stages
                ),
                key=lambda pair: (pair[1].sequence, pair[0]),
            )
            if options.enabled
            else []
        )
        fragment_meta: list[dict[str, object]] = []
        vector_meta: list[dict[str, object]] = []
        resolved_fragments: dict[str, dict[str, object]] = {}
        resolved_texts: dict[str, str] = {}
        for _, fragment in fragments:
            if fragment.content is not None:
                artifact = await self.artifacts.get(fragment.content.artifact)
                selected = ContentSelection(
                    artifact=artifact.ref, selection=fragment.content.selector
                )
                selected = (await self.artifacts.normalize_selections((selected,)))[0]
            else:
                assert fragment.text is not None
                selected = fragment.text
            if fragment.stage == "retrieval" and (uses_contextual_signal or not raw_present):
                query_selections.append(selected)
            resolved_texts[fragment.key] = (
                await self.artifacts.read_selection(selected)
                if isinstance(selected, ContentSelection)
                else selected
            )
            resolved_fragments[fragment.key] = {
                "key": fragment.key,
                "role": fragment.role,
                "stage": fragment.stage,
                "parent_key": fragment.parent_key or fragment.key,
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
                    "stage": fragment.stage,
                    "parent_key": fragment.parent_key or fragment.key,
                    "sequence": fragment.sequence,
                    "weight": fragment.weight,
                    "relevance": fragment.relevance,
                    "validity": fragment.validity,
                    "distance": fragment.distance,
                    "half_life": fragment.half_life,
                    "source_key": (
                        selected.artifact.identifier
                        if isinstance(selected, ContentSelection)
                        else hashlib.sha256(selected.encode("utf-8")).hexdigest()
                    ),
                    "source_start": selected.selection.start_byte
                    if isinstance(selected, ContentSelection)
                    else None,
                    "source_end": selected.selection.end_byte
                    if isinstance(selected, ContentSelection)
                    else None,
                }
            )
            if fragment.stage == "retrieval" and (uses_contextual_signal or not raw_present):
                vector_meta.append(fragment_meta[-1])
        aggregate_options: dict[str, object] = {
            "window": options.window,
            "context_mix": options.mix,
            "null_mass": options.null_mass,
            "role_weights": dict(options.role_weights),
            "default_role_weight": options.default_role_weight,
        }
        mass = (
            _ContextDiagnostics.model_validate_json(
                await self.execution.blocking(
                    native.context_masses,
                    encode_json(fragment_meta),
                    encode_json(aggregate_options),
                )
            )
            if fragment_meta
            else _ContextDiagnostics()
        )
        selected_context = [
            {
                **resolved_fragments[item.key],
                "decayed_weight": item.decayed_weight,
                "role_weight": item.role_weight,
                "contribution": item.contribution,
            }
            for item in mass.selected_context
        ]
        lexical_context = [
            {"key": item.key, "text": resolved_texts[item.key], "contribution": item.contribution}
            for item in mass.selected_context
            if item.stage == "retrieval"
        ]
        localization_context = [
            {"key": item.key, "text": resolved_texts[item.key], "contribution": item.contribution}
            for item in mass.selected_context
            if item.stage == "localization"
        ]
        evaluation_context = [
            resolved_texts[item.key] for item in mass.selected_context if item.stage == "evaluation"
        ]
        if not needs_vectors:
            return (
                None,
                None,
                None,
                {
                    "query_origin": "text",
                    "context_used": bool(mass.selected_context),
                    "selected_context": selected_context,
                    "roles": list(mass.roles),
                    "omitted_context_count": len(bound.context) - len(mass.selected_context),
                    "core_keys": [concept["key"] for concept in resolved_core],
                    "core_labels": [concept["label"] for concept in resolved_core],
                    "context_lexical": lexical_context,
                    "localization_context": localization_context,
                    "evaluation_context": evaluation_context,
                },
            )
        if space_id is None:
            raise NousError("wave.capability_unavailable", "This snapshot has no vector space")
        concept_only = not raw_present and not vector_meta
        if concept_only:
            for core_index, concept in enumerate(resolved_core):
                text = Concept.model_validate(concept).embedding_text()
                query_selections.append(text)
                vector_meta.append(
                    {
                        "key": f"core:{core_index}",
                        "role": "concept",
                        "sequence": len(vector_meta),
                        "weight": 1.0,
                    }
                )
            for cue_index, cue in enumerate(request.cues):
                query_selections.append(cue.embedding_text())
                vector_meta.append(
                    {
                        "key": f"cue:{cue_index}",
                        "role": "concept",
                        "sequence": len(vector_meta),
                        "weight": 1.0,
                    }
                )
        if not query_selections and raw_vector is None:
            raise NousError(
                "wave.empty_query", "Provide query text, context, a core concept or a cue"
            )
        context_selections = query_selections[1:] if encoded_raw else query_selections
        context_key = tuple(
            item.model_dump_json() if isinstance(item, ContentSelection) else "text:" + item
            for item in context_selections
        )
        cached_context = context_cache.get(context_key) if context_cache and context_key else None
        if cached_context is not None:
            raw = raw_vector
            if encoded_raw:
                raw = (await self.embedding.encode_query((request.text,), space_id))[0]
            context_vectors = cached_context
        else:
            vectors = (
                await self.embedding.encode_query(query_selections, space_id)
                if query_selections
                else np.empty(
                    (0, len(raw_vector) if raw_vector is not None else 0), dtype=np.float32
                )
            )
            raw = raw_vector if raw_vector is not None else (vectors[0] if encoded_raw else None)
            context_vectors = vectors[1:] if encoded_raw else vectors
            if context_cache is not None and context_key:
                context_cache[context_key] = context_vectors
        if len(context_vectors):
            if concept_only:
                aggregate_options.update(
                    {
                        "window": len(context_vectors),
                        "context_mix": 1.0,
                        "role_weights": {"concept": 1.0},
                    }
                )
            aggregated = await self.execution.blocking(
                native.aggregate_context,
                raw,
                context_vectors,
                encode_json(vector_meta),
                encode_json(aggregate_options),
            )
            effective = aggregated.vector
            diagnostic = _ContextDiagnostics.model_validate_json(aggregated.diagnostics_json)
            summary: dict[str, object] = diagnostic.model_dump(
                mode="json", exclude={"selected_context", "effective_query"}, exclude_none=True
            )
            summary["context_options"] = aggregate_options
            if concept_only:
                summary["query_origin"] = "concepts"
        else:
            effective = raw
            diagnostic = _ContextDiagnostics()
            summary = {"query_origin": "text", "context_used": bool(mass.selected_context)}
        summary["selected_context"] = selected_context
        summary["roles"] = list(mass.roles)
        summary["context_lexical"] = lexical_context
        summary["localization_context"] = localization_context
        summary["evaluation_context"] = evaluation_context
        summary["omitted_context_count"] = len(bound.context) - len(selected_context)
        ghosts: npt.NDArray[np.float32] | None = None
        if request.cues and include_ghosts:
            ghosts = await self.embedding.encode_query(
                tuple(cue.embedding_text() for cue in request.cues), space_id
            )
        if bound.vector is not None:
            summary["query_origin"] = "vector+context" if len(context_vectors) else "vector"
            summary["vector_ref"] = {
                "artifact": bound.vector.artifact.identifier,
                "row_index": bound.vector.row_index,
            }
        summary["core_keys"] = [concept["key"] for concept in resolved_core]
        summary["core_labels"] = [concept["label"] for concept in resolved_core]
        return raw if raw is not None else effective, effective, ghosts, summary

    async def prepare(
        self, request: WaveQuery, row: SnapshotRow, capabilities: Sequence[str]
    ) -> QueryPlan:
        bound = require_bound_query(request)
        await self.time.validate(fragment.time for fragment in bound.context)
        if bound.candidate_filter is not None:
            await self.time.validate(condition.range for condition in bound.candidate_filter.times)
        wave = self.execution.snapshot.values.wave
        program = resolve_program(
            wave, request.program or (wave.exploration_program if request.explore else None)
        )
        selection: Literal["explicit", "default", "fallback", "exploration"] = (
            "explicit" if request.program else "exploration" if request.explore else "default"
        )
        missing = program.requirements - set(capabilities)
        if missing and request.program is None and not request.explore and wave.unavailable_program:
            program = resolve_program(wave, wave.unavailable_program)
            selection = "fallback"
            missing = program.requirements - set(capabilities)
        if missing:
            raise NousError(
                "wave.capability_unavailable",
                "Snapshot cannot execute the requested program",
                details={"program": program.name, "missing": sorted(missing)},
            )
        if (bound.thinking_stages or request.explore) and "thinking" not in program.outputs:
            raise NousError(
                "wave.capability_unavailable",
                "The selected program does not produce thinking routes",
                details={"program": program.name},
            )
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
        allowed = allowed_rows(mapping, bound.candidate_filter)

        def leaf_texts(expression: WaveExpression, prefix: tuple[str, ...] = ()) -> list[str]:
            parts = (*prefix, expression.text) if expression.text.strip() else prefix
            if expression.kind == "term":
                return [" ".join(parts).strip()]
            return [text for child in expression.children for text in leaf_texts(child, parts)]

        expression = request.expression
        if expression is None and (request.prefer or request.avoid or request.sort):
            expression = WaveExpression(
                kind="term", text=request.text, candidate_filter=bound.candidate_filter
            )
        expression_texts = leaf_texts(expression) if expression else []
        localization_text = (
            request.text.strip() or " ".join(dict.fromkeys(expression_texts)).strip()
        )
        signal_request = request.model_copy(update={"text": localization_text})
        context_cache: dict[tuple[str, ...], npt.NDArray[np.float32]] = {}
        raw_vector, contextual_vector, ghosts, query_summary = await self._query_inputs(
            signal_request,
            row,
            program,
            vectors_required=False
            if not localization_text and not request.core_concepts and not request.cues
            else None,
            context_cache=context_cache,
        )
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
            "candidates_per_group": controls["candidates_per_group"],
            "ann_search_effort": controls["ann_search_effort"],
            "group_scan_limit": wave.query.group_scan_limit,
            "include_trace": request.include_trace,
            "core_rows": core_rows,
            "ghosts": [{"name": cue.label, "is_core": cue.is_core} for cue in request.cues],
            "thinking_stages": stages,
            "explore": request.explore,
            "context_lexical": query_summary.get("context_lexical", []),
            "localization_context": query_summary.get("localization_context", []),
            "dedup_threshold": wave.dedup.threshold if wave.dedup.semantic_enabled else None,
            "rrf_k": wave.fusion.rrf_k,
            "preference_weight": controls["preference_weight"],
            "include_counterevidence": (
                request.include_counterevidence
                if request.include_counterevidence is not None
                else wave.query.include_counterevidence
            ),
        }
        query_text = " ".join([localization_text, *(cue.label for cue in request.cues)]).strip()
        if not query_text and "text" in program.requirements:
            query_text = " ".join(cast(list[str], query_summary.get("core_labels", [])))
        if not query_text and raw_vector is None and expression is None:
            raise NousError(
                "wave.empty_query", "Lexical retrieval requires query text or cue labels"
            )
        native_candidates = None
        if bound.candidate_set is not None:
            if expression is not None:
                raise NousError(
                    "wave.invalid_input",
                    "A fixed candidate set cannot be combined with a query expression",
                )
            if not any(
                ref.node == "scope" and ref.port == "candidates"
                for node in program.nodes
                for value in node.inputs.values()
                for ref in (value if isinstance(value, tuple) else (value,))
            ):
                raise NousError(
                    "wave.invalid_input",
                    "The selected program does not consume a fixed candidate set",
                )
            candidate_set = bound.candidate_set
            if candidate_set.snapshot not in {row["identifier"], row["id"]}:
                raise NousError(
                    "wave.candidate_set_mismatch", "Candidate set belongs to another snapshot"
                )
            unknown = {entry.candidate for entry in candidate_set.entries} - candidate_rows.keys()
            if unknown:
                raise NousError(
                    "wave.candidate_not_found",
                    "Candidate set contains unknown keys",
                    details={"candidates": sorted(unknown)},
                )
            import nous_native as native

            native_candidates = native.CandidateSet(
                encode_json(
                    {
                        "identity": row["identity"],
                        "rank_scope": candidate_set.rank_scope,
                        "entries": [
                            {
                                "row": candidate_rows[entry.candidate],
                                "lanes": {
                                    name: lane.model_dump(mode="json")
                                    for name, lane in entry.lanes.items()
                                },
                            }
                            for entry in candidate_set.entries
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
        selection_node = next(
            (node for node in program.nodes if node.operator == "passages.select"), None
        )
        configured_budget = selection_node.parameters.get("budget") if selection_node else None
        budget = request.delivery_budget or (
            DeliveryBudget.model_validate(configured_budget)
            if configured_budget is not None
            else None
        )
        if budget is not None and budget.unit == "tokens":
            raise NousError(
                "wave.capability_unavailable", "A tokenizer is required for token budgets"
            )
        signature = row["identity"].get("space_signature")
        evaluation_tasks: dict[str, object] = {}
        for node in program.nodes:
            if node.operator != "passages.evaluate":
                continue
            parameters = EvaluationParameters.model_validate(node.parameters)
            task = parameters.task or (
                "rank-passages" if parameters.method == "rank" else "evaluate-passages"
            )
            assignment = self.execution.snapshot.values.tasks.get(task)
            evaluation_tasks[task] = (
                assignment.model_dump(mode="json") if assignment is not None else None
            )
        program_run = program_run_facts(
            program,
            requested_program=request.program,
            selection=selection,
            options=search_options,
            context_options=wave.context.model_dump(mode="json"),
            evaluation_options=self.execution.snapshot.values.evaluation.model_dump(mode="json"),
            evaluation_tasks=evaluation_tasks,
            budget=budget,
            space_id=row["space_id"],
            space_signature=signature if isinstance(signature, str) else None,
            space_definition=row["space_definition"],
            candidate_set=bound.candidate_set,
            expression_kind=expression.kind if expression is not None else None,
            expression_terms=len(expression_texts),
        )
        query_summary["grouping"] = {
            "by": request.grouping,
            "top_k": request.top_k or wave.query.top_k,
            "passages_per_group": request.passages_per_group,
            "lane_limit_unit": "groups" if request.grouping == "document" else "candidates",
            "group_scan_limit": wave.query.group_scan_limit,
        }
        sort_requests: set[str] = {request.sort} if request.sort else set()

        def gather_sorts(value: WaveExpression) -> None:
            if value.sort:
                sort_requests.add(value.sort)
            for child in value.children:
                gather_sorts(child)

        if expression is not None:
            gather_sorts(expression)
        sort_values: dict[str, dict[str, object]] = {}
        for sort in sort_requests:
            parts = sort.split(",")
            if (
                len(parts) > 2
                or parts[0] not in {"occurred", "observed", "formed", "valid", "recorded"}
                or (len(parts) == 2 and parts[1] not in {"asc", "desc"})
            ):
                raise NousError(
                    "wave.invalid_input", "Invalid time ordering", details={"sort": sort}
                )
            axis = parts[0]
            values: dict[str, object] = {}
            for group_key, descriptor in mapping["groups"].items():
                group = GroupDescriptor.model_validate(descriptor)
                document = mapping["documents"][group.document_key]
                times = (
                    CognitiveTimes.model_validate(document["times"]) if document["times"] else None
                )
                value = getattr(times, axis) if times else None
                point: TimePoint | None = (
                    value
                    if isinstance(value, TimePoint)
                    else value.at
                    if isinstance(value, Instant)
                    else value.start.at
                    if isinstance(value, Interval) and isinstance(value.start, Known)
                    else None
                )
                values[group_key] = (
                    {"domain": point.domain_id, "ticks": point.ticks} if point else None
                )
            sort_values[axis] = values
        search_options["sort_values"] = sort_values
        expression_plan: dict[str, object] | None = None
        branch_raw: list[npt.NDArray[np.float32]] = []
        branch_contextual: list[npt.NDArray[np.float32]] = []
        if expression is not None:
            branches: list[dict[str, object]] = []
            vectors: dict[tuple[str, tuple[str, ...]], int] = {}

            async def bind_expression(
                value: WaveExpression, path: str, prefix: tuple[str, ...] = ()
            ) -> dict[str, object]:
                parts = (*prefix, value.text) if value.text.strip() else prefix
                if value.kind != "term":
                    return {
                        "kind": value.kind,
                        "limit": value.limit,
                        "sort": value.sort,
                        "prefer": list(value.prefer),
                        "avoid": list(value.avoid),
                        "children": [
                            await bind_expression(child, f"{path}.{index}", parts)
                            for index, child in enumerate(value.children)
                        ],
                    }
                text = " ".join(parts).strip()
                if (
                    not text
                    and value.candidate_filter is None
                    and not value.core_concepts
                    and bound.candidate_filter is None
                ):
                    raise NousError(
                        "wave.empty_query", "A query term needs text or a bound selector"
                    )
                branch_allowed = set(allowed)
                if value.candidate_filter is not None:
                    await self.time.validate(
                        condition.range for condition in value.candidate_filter.times
                    )
                    branch_allowed.intersection_update(
                        allowed_rows(mapping, value.candidate_filter)
                    )
                concepts_for_branch = value.core_concepts or request.core_concepts
                branch_request = signal_request.model_copy(
                    update={
                        "text": text,
                        "core_concepts": concepts_for_branch,
                        "expression": None,
                    }
                )
                index = None
                branch_summary: dict[str, object] = {}
                if text or concepts_for_branch:
                    key = (text, concepts_for_branch)
                    if text == localization_text and concepts_for_branch == request.core_concepts:
                        branch_vector, branch_effective, branch_summary = (
                            raw_vector,
                            contextual_vector,
                            query_summary,
                        )
                    else:
                        (
                            branch_vector,
                            branch_effective,
                            _,
                            branch_summary,
                        ) = await self._query_inputs(
                            branch_request,
                            row,
                            program,
                            context_cache=context_cache,
                            include_ghosts=False,
                        )
                    if branch_vector is not None and branch_effective is not None:
                        if key not in vectors:
                            vectors[key] = len(branch_raw)
                            branch_raw.append(branch_vector)
                            branch_contextual.append(branch_effective)
                        index = vectors[key]
                keys_for_branch = branch_summary.get("core_keys", [])
                if concepts_for_branch:
                    required = set(cast(list[str], keys_for_branch))
                    branch_allowed = {
                        candidate_row
                        for candidate_row in branch_allowed
                        if required.issubset(set(candidates[candidate_row]["concepts"]))
                    }
                branch_index = len(branches)
                branches.append(
                    {
                        "name": path,
                        "text": text,
                        "allowed_rows": sorted(branch_allowed),
                        "core_rows": [keys[key] for key in cast(list[str], keys_for_branch)],
                        "vector_index": index,
                        "limit": value.limit,
                        "prefer": list(value.prefer),
                        "avoid": list(value.avoid),
                        "sort": value.sort,
                    }
                )
                return {"kind": "term", "branch": branch_index, "limit": value.limit}

            tree = await bind_expression(expression, "0")
            expression_plan = {
                "tree": tree,
                "branches": branches,
                "prefer": list(request.prefer),
                "avoid": list(request.avoid),
                "sort": request.sort,
            }
        return QueryPlan(
            program,
            query_text,
            raw_vector,
            contextual_vector,
            ghosts,
            np.asarray(allowed, dtype=np.uint64),
            search_options,
            native_candidates,
            query_summary,
            program_run,
            expression_plan,
            np.stack(branch_raw).astype(np.float32) if branch_raw else None,
            np.stack(branch_contextual).astype(np.float32) if branch_contextual else None,
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
