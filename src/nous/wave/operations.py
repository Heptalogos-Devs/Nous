"""Complete Wave use cases over immutable snapshots and source-aware query planning."""

from __future__ import annotations

import copy
import json
from collections.abc import Sequence
from contextlib import AbstractAsyncContextManager
from time import perf_counter
from typing import TYPE_CHECKING

from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import CallContext, EventDeliveryError, define_operation
from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.common.operations import OperationIssue, record_completed, report_outcome
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, ExternalInput
from nous.content.results import ResultStore
from nous.content.selections import ordered_selections
from nous.media.preparation import MediaPreparer
from nous.models.embeddings import resolved_space
from nous.models.evaluation import EvaluationOperations
from nous.models.evaluation_contracts import (
    CandidateEvaluation,
    EvaluationCandidate,
    EvaluationOutcome,
    EvaluationTask,
    RankRequest,
)
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
    CandidateRef,
    CandidateSet,
    ContextFragment,
    CorpusView,
    DeliveryBudget,
    GroupDescriptor,
    ResolvedBuildProfile,
    ReusableDocument,
    ScoreValue,
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
from .delivery import (
    LocatedGroup,
    LocatedHit,
    LocatedResult,
    PassageProposal,
    WaveDelivery,
    select_passages,
)
from .program import CompiledProgram, EvaluationParameters, native_segment
from .query import WaveQueries, require_bound_query
from .reading import describe_neighborhood, neighborhood_rows, read_candidate
from .serialization import encode_json
from .snapshots import WaveSnapshots, build_profile, view_profile

if TYPE_CHECKING:
    import nous_native

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
        evaluator: EvaluationOperations | None = None,
    ) -> None:
        self.execution, self.database = execution, database
        self.snapshots = WaveSnapshots(
            database, execution, paths, instance_id, time, artifacts, results, embedding, preparer
        )
        self.queries = WaveQueries(execution, artifacts, embedding, time, instance_id)
        self.delivery = WaveDelivery(artifacts, instance_id)
        self.evaluator = evaluator

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

    async def group_candidates(
        self, ref: str | SnapshotRef
    ) -> dict[str, tuple[GroupDescriptor, tuple[str, ...]]]:
        """Read group identities and their candidate keys from one frozen mapping."""
        async with self.execution.operation("wave.group_candidates"):
            async with self.snapshots.pin(ref) as row:
                mapping = row["mapping"]
                result: dict[str, list[str]] = {key: [] for key in mapping["groups"]}
                for candidate in mapping["candidates"]:
                    result[candidate["group_key"]].append(candidate["key"])
                return {
                    key: (GroupDescriptor.model_validate(mapping["groups"][key]), tuple(keys))
                    for key, keys in result.items()
                }

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
            "candidate_set": TypeAdapter(CandidateSet),
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
        return await self.query(WaveQuery.model_validate(values), call=call)

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

    async def query(
        self, request: WaveQuery, *, call: CallContext | None = None
    ) -> WaveSearchResult:
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
                plan = await self.queries.prepare(request, row, snapshot.capabilities)
                locator, native_plan = native_segment(plan.program)

                def compute() -> nous_native.NativeQueryResult:
                    if plan.expression is None:
                        return snapshot.execute_program(
                            name=plan.program.name,
                            program_json=encode_json(native_plan),
                            text=plan.text,
                            raw_vector=plan.raw_vector,
                            contextual_vector=plan.contextual_vector,
                            ghost_vectors=plan.ghosts,
                            allowed_rows=plan.allowed,
                            options_json=encode_json(plan.options),
                            candidate_set=plan.candidate_set,
                            cancel=token,
                        )
                    return snapshot.execute_expression(
                        name=plan.program.name,
                        program_json=encode_json(native_plan),
                        expression_json=encode_json(plan.expression),
                        text=plan.text,
                        original_vector=plan.raw_vector,
                        raw_vectors=plan.branch_raw_vectors,
                        contextual_vectors=plan.branch_contextual_vectors,
                        ghost_vectors=plan.ghosts,
                        allowed_rows=plan.allowed,
                        options_json=encode_json(plan.options),
                        cancel=token,
                    )

                result = await self.snapshots.compute(compute, token)
                output = await self.delivery.locate(
                    request, row, result, plan.summary, plan.program_run
                )
            output = await self._finish_query(plan.program, locator, request, output, call)
            get_logger(__name__).info(
                "wave.query.completed",
                extra={
                    "snapshot": row["identifier"],
                    "program": plan.program.name,
                    "returned": len(output.groups or output.hits),
                    "lanes": output.used_lanes,
                },
            )
            return copy.deepcopy(output)

    async def _evaluate_passages(
        self,
        result: LocatedResult,
        request: WaveQuery,
        parameters: EvaluationParameters,
        call: CallContext | None,
    ) -> LocatedResult:
        passages = (
            [part for group in result.groups for part in group.passages]
            if result.groups
            else [hit.passage for hit in result.hits]
        )
        if not passages:
            return result
        try:
            if self.evaluator is None or call is None:
                raise NousError(
                    "model.capability_missing",
                    "Passage evaluation requires an application call and evaluator",
                )
            evaluation_context = result.result.context_summary.get("evaluation_context", [])
            selected_context = (
                "\n\n".join(item for item in evaluation_context if isinstance(item, str))
                if isinstance(evaluation_context, list)
                else ""
            )
            settings = call.settings.values.evaluation

            def cost(part: PassageProposal) -> int:
                size = len(selected_context)
                for content in (*part.fragments, *part.support_context):
                    view = result.views[content.artifact.id]
                    start, end = view.selection_bounds(content)
                    size += len(
                        view.raw[start - view.byte_offset : end - view.byte_offset].decode("utf-8")
                    )
                return size

            def relevance(item: tuple[int, PassageProposal]) -> tuple[float, int]:
                index, part = item
                score = next(
                    (value.value for value in part.scores if value.name == "local_relevance"), 0.0
                )
                return -score, index

            chosen: list[tuple[int, PassageProposal]] = []
            used = 0
            for index, part in sorted(enumerate(passages), key=relevance):
                size = cost(part)
                if len(chosen) >= settings.max_candidates or used + size > settings.max_characters:
                    continue
                chosen.append((index, part))
                used += size
            if not chosen:
                raise NousError(
                    "model.input_limit", "No localized passage fits the evaluator budget"
                )
            ordered = [
                ordered_selections((*part.fragments, *part.support_context)) for _, part in chosen
            ]
            references = tuple(content for selections in ordered for content in selections)
            readings = iter(await self.delivery.artifacts.read_selections(references))
            inputs: list[EvaluationCandidate] = []
            for (index, _part), selections in zip(chosen, ordered, strict=True):
                pieces: list[str] = []
                previous: ContentSelection | None = None
                for content in selections:
                    if previous is not None and (
                        previous.artifact != content.artifact
                        or previous.selection.end_byte != content.selection.start_byte
                    ):
                        pieces.append("…")
                    pieces.append(next(readings).text)
                    previous = content
                inputs.append(
                    EvaluationCandidate(
                        key=f"p{index}", text="".join(pieces), context=selected_context or None
                    )
                )
            candidates = tuple(inputs)
            task = parameters.task or (
                "rank-passages" if parameters.method == "rank" else "evaluate-passages"
            )
            if parameters.method == "rank":
                outcome = await self.evaluator.rank(
                    RankRequest(question=request.text, candidates=candidates, task=task), call
                )
            else:
                outcome = await self.evaluator.evaluate(
                    EvaluationTask(
                        question=request.text,
                        candidates=candidates,
                        rubric=parameters.rubric,
                        purpose=parameters.purpose,
                        task=task,
                        scale=parameters.scale,
                    ),
                    call,
                )
            observed = [item.key for item in outcome.items]
            if len(observed) != len(set(observed)) or not set(observed).issubset(
                {item.key for item in candidates}
            ):
                raise NousError(
                    "model.invalid_output", "Evaluation returned unknown or repeated passage keys"
                )
            assessments: dict[str, tuple[int | None, dict[str, JsonValue]]] = {}
            if isinstance(outcome, EvaluationOutcome) and (
                not parameters.scale.labels or parameters.category_order
            ):

                def evaluation_order(item: CandidateEvaluation) -> float:
                    if isinstance(item.value, str):
                        return float(parameters.category_order.index(item.value))
                    return -item.value if parameters.numeric_order == "higher" else item.value

                previous_value: float | None = None
                rank = 0
                for index, item in enumerate(sorted(outcome.items, key=evaluation_order), start=1):
                    value = evaluation_order(item)
                    if previous_value is None or value != previous_value:
                        rank = index
                        previous_value = value
                    assessments[item.key] = (rank, item.model_dump(mode="json"))
            elif isinstance(outcome, EvaluationOutcome):
                assessments = {
                    item.key: (None, item.model_dump(mode="json")) for item in outcome.items
                }
            else:
                assessments = {
                    item.key: (rank, item.model_dump(mode="json"))
                    for rank, item in enumerate(outcome.items, start=1)
                }

            def add_assessment(index: int, passage: PassageProposal) -> PassageProposal:
                assessed = assessments.get(f"p{index}")
                if assessed is None:
                    return passage
                rank, assessment = assessed
                numeric = assessment.get("score", assessment.get("value"))
                scores = passage.scores
                if isinstance(numeric, int | float) and not isinstance(numeric, bool):
                    scores = (
                        *scores,
                        ScoreValue(
                            name="model_relevance",
                            value=float(numeric),
                            scale=str(assessment.get("scale", "model")),
                            producer="passages.evaluate",
                        ),
                    )
                if rank is not None:
                    scores = (
                        *scores,
                        ScoreValue(
                            name="model_rank",
                            value=float(rank),
                            scale="rank",
                            producer="passages.evaluate",
                            based_on=("assessment",),
                        ),
                    )
                return passage.model_copy(update={"assessment": assessment, "scores": scores})

            index = 0
            groups: list[LocatedGroup] = []
            for group in result.groups:
                updated: list[PassageProposal] = []
                for part in group.passages:
                    updated.append(add_assessment(index, part))
                    index += 1
                groups.append(group.model_copy(update={"passages": tuple(updated)}))
            hits: list[LocatedHit] = []
            for hit in result.hits:
                hits.append(hit.model_copy(update={"passage": add_assessment(index, hit.passage)}))
                index += 1
            summary = dict(result.result.context_summary)
            summary["evaluation"] = {
                "method": parameters.method,
                "numeric_order": parameters.numeric_order,
                "category_order": list(parameters.category_order),
                "task": task,
                "producer": outcome.producer.model_dump(mode="json"),
                "usage": outcome.usage,
                "actual_input": outcome.actual_input.model_dump(mode="json"),
                "batches": [batch.model_dump(mode="json") for batch in outcome.batches],
                "items": [item.model_dump(mode="json") for item in outcome.items],
                "candidates": [
                    {
                        "key": f"p{index}",
                        "candidate": part.candidate.model_dump(mode="json"),
                        "scored_ranges": [
                            value.model_dump(mode="json") for value in part.scored_ranges
                        ],
                        "support_context": [
                            value.model_dump(mode="json") for value in part.support_context
                        ],
                    }
                    for index, part in chosen
                ],
                "evaluated": len(outcome.items),
                "offered": len(passages),
            }
            return result.model_copy(
                update={
                    "groups": tuple(groups),
                    "hits": tuple(hits),
                    "result": result.result.model_copy(update={"context_summary": summary}),
                }
            )
        except EventDeliveryError, TimeoutError:
            raise
        except Exception as error:
            if parameters.policy == "required":
                raise
            issue = OperationIssue(
                stage="passages.evaluate",
                code=error.code if isinstance(error, NousError) else "model.evaluation_failed",
                message=str(error),
                details=dict(error.details) if isinstance(error, NousError) else {},
                retryable=error.retryable if isinstance(error, NousError) else False,
            )
            report_outcome("partial", issues=(issue,))
            return result.model_copy(
                update={
                    "result": result.result.model_copy(
                        update={"issues": (*result.result.issues, issue.to_dict())}
                    )
                }
            )

    async def _finish_query(
        self,
        program: CompiledProgram,
        locator: str,
        request: WaveQuery,
        localized: LocatedResult,
        call: CallContext | None,
    ) -> WaveSearchResult:
        values: dict[tuple[str, str], LocatedResult | WaveSearchResult] = {
            (locator, "passages"): localized
        }
        for node in program.nodes:
            if node.operator not in {"passages.evaluate", "passages.select", "delivery.read"}:
                continue
            input_port = {
                "passages.evaluate": "passages",
                "passages.select": "passages",
                "delivery.read": "selected",
            }[node.operator]
            ref = node.inputs[input_port]
            if isinstance(ref, tuple) or (ref.node, ref.port) not in values:
                raise NousError(
                    "wave.invalid_program",
                    "Delivery input is unavailable",
                    details={"node": node.id, "port": input_port},
                )
            value = values[(ref.node, ref.port)]
            if not isinstance(value, LocatedResult):
                raise NousError(
                    "wave.invalid_program",
                    "Delivery stage requires localized ranges",
                    details={"node": node.id},
                )
            if node.operator == "passages.evaluate":
                output: LocatedResult | WaveSearchResult = await self._evaluate_passages(
                    value, request, EvaluationParameters.model_validate(node.parameters), call
                )
                output_port = "passages"
            elif node.operator == "passages.select":
                configured = node.parameters.get("budget")
                budget = request.delivery_budget or (
                    DeliveryBudget.model_validate(configured) if configured is not None else None
                )
                output = select_passages(value, request.passages_per_group, budget)
                output_port = "selected"
            else:
                output = await self.delivery.read(value)
                output_port = "evidence"
            values[(node.id, output_port)] = output
        output_ref = program.outputs["evidence"]
        try:
            final = values[(output_ref.node, output_ref.port)]
        except KeyError as error:
            raise NousError("wave.invalid_program", "Program did not produce evidence") from error
        if not isinstance(final, WaveSearchResult):
            raise NousError("wave.invalid_program", "Program evidence was not read")
        return final

    async def read(self, ref: str, candidate_key: str, *, sources: bool = False) -> WaveReadResult:
        async with self.execution.operation("wave.read"):
            async with self.snapshots.pin(ref) as row:
                return copy.deepcopy(
                    await read_candidate(
                        row,
                        candidate_key,
                        self.delivery.artifacts,
                        self.delivery.instance_id,
                        sources=sources,
                    )
                )

    async def neighborhood(self, ref: str, concepts: Sequence[str]) -> WaveNeighborhoodResult:
        async with self.execution.operation("wave.neighborhood"):
            async with self.snapshots.borrow(ref) as (row, snapshot, token):
                rows = neighborhood_rows(row, concepts)
                result = await self.snapshots.compute(
                    lambda: snapshot.neighborhood(rows, cancel=token), token
                )
                return copy.deepcopy(
                    describe_neighborhood(row, concepts, result, self.delivery.instance_id)
                )

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
