"""Coordinate current projections and return the actual grouped cognitive delivery."""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass
from time import perf_counter
from typing import cast

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.results import ResultStore
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, WriteTx
from nous.wave.contracts import (
    CandidateRef,
    CognitionOwner,
    SearchCoverage,
    SelectedContext,
    SnapshotDescriptor,
    WaveQuery,
)
from nous.wave.operations import WaveOperations

from .. import basis, schema
from ..annotations import AnnotationOperations
from ..contracts import Namespace, RecordRevisionRef, SourceBinding
from ..scope import NamespaceOperations
from ..writing.reading import RecordReader
from .capture import CorpusCapture
from .contracts import (
    CognitiveQuery,
    DeliveredSelection,
    Mode,
    QueryContext,
    QueryOperationContext,
    QueryResult,
    SearchGroup,
    SearchMode,
    SearchPassage,
)
from .projection import ProjectionBuilder

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class SearchOptions:
    namespace: str
    mode: SearchMode
    space: str | None
    include_review: bool
    include_exploration: bool
    candidate_limit: int


class CognitiveRetrieval:
    """Coordinate current-view capture, publication and grouped cognitive retrieval.

    Wave owns preparation, ranking and delivery. A bound instance restricts requests
    to one namespace. Current queries can build and publish a snapshot when needed.
    """

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        scope: NamespaceOperations,
        records: RecordReader,
        artifacts: ArtifactStore,
        results: ResultStore,
        wave: WaveOperations,
        annotations: AnnotationOperations,
        *,
        namespace: Namespace | None = None,
        locks: dict[tuple[str, str], asyncio.Lock] | None = None,
    ):
        self.database, self.execution, self.scope = database, execution, scope
        self.records, self.artifacts, self.results, self.wave = records, artifacts, results, wave
        self.annotations, self.namespace = annotations, namespace
        self.capture = CorpusCapture(scope, records, artifacts, annotations)
        self.projection = ProjectionBuilder(results)
        self._locks = locks if locks is not None else {}

    def bind(self, namespace: Namespace) -> CognitiveRetrieval:
        """Return a namespace-bound retrieval service sharing the build locks."""
        return CognitiveRetrieval(
            self.database,
            self.execution,
            self.scope,
            self.records,
            self.artifacts,
            self.results,
            self.wave,
            self.annotations,
            namespace=namespace,
            locks=self._locks,
        )

    def _options(
        self,
        namespace: str | None,
        mode: Mode | None,
        include_review: bool | None,
        include_exploration: bool | None,
    ) -> SearchOptions:
        settings = self.execution.snapshot.typed_values().cognition
        if self.namespace is not None:
            if namespace is not None and namespace not in {self.namespace.name, self.namespace.id}:
                raise NousError(
                    "cognition.namespace_mismatch", "Retrieval is bound to another namespace"
                )
            namespace = self.namespace.id
        selected = mode or settings.search.default_mode
        if selected == "auto":
            selected = "hybrid" if settings.search.space else "lexical"
        space = None if selected == "lexical" else settings.search.space
        if selected != "lexical" and not space:
            raise NousError(
                "cognition.space_required", "This retrieval mode requires an embedding space"
            )
        return SearchOptions(
            namespace or settings.default_namespace,
            selected,
            space,
            settings.search.include_review if include_review is None else include_review,
            settings.search.include_exploration
            if include_exploration is None
            else include_exploration,
            settings.search.local_candidate_max_bytes,
        )

    async def _current_view(self, selected: SearchOptions) -> tuple[Namespace, SnapshotDescriptor]:
        policy: dict[str, object] = {
            "include_review": selected.include_review,
            "include_exploration": selected.include_exploration,
            "local_candidate_max_bytes": selected.candidate_limit,
        }
        profile = self.wave.profile(selected.space, policy)
        async with self.database.read() as tx:
            ns = await self.scope.get_in(tx, selected.namespace)
        collection = f"cognition:{ns.id}:{profile}"
        existing = await self.wave.current(collection)
        if (
            existing is not None
            and existing.captured_generation == ns.content_generation
            and existing.profile_signature == profile
        ):
            self._check_owner(existing, ns)
            return ns, existing
        async with self._locks.setdefault((ns.id, profile), asyncio.Lock()):
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, ns.id)
            existing = await self.wave.current(collection)
            if (
                existing is not None
                and existing.captured_generation == ns.content_generation
                and existing.profile_signature == profile
            ):
                self._check_owner(existing, ns)
                return ns, existing
            captured_at = perf_counter()
            async with self.database.read() as tx:
                captured = await self.capture.capture_in(tx, ns.id)
            capture_ms = (perf_counter() - captured_at) * 1000
            projection_at = perf_counter()
            view = await self.projection.build(
                captured,
                collection,
                profile,
                include_review=selected.include_review,
                include_exploration=selected.include_exploration,
                candidate_limit=selected.candidate_limit,
            )

            logger.info(
                "cognition.capture.completed",
                extra={
                    "namespace_id": ns.id,
                    "generation": captured.namespace.content_generation,
                    "records": len(captured.current),
                    "capture_ms": capture_ms,
                    "projection_ms": (perf_counter() - projection_at) * 1000,
                },
            )

            async def unchanged(tx: WriteTx) -> bool:
                return (
                    await self.scope.get_in(tx, ns.id)
                ).content_generation == captured.namespace.content_generation

            try:
                published = await self.wave.build_view(view, selected.space, publish_if=unchanged)
            except NousError as error:
                raise NousError(
                    "cognition.projection_failed",
                    "The current retrieval view could not be built",
                    details={
                        "namespace": ns.name,
                        "generation": captured.namespace.content_generation,
                        "cause": error.to_dict(),
                        "retry": "journal refresh",
                    },
                    retryable=error.retryable,
                    cause=error,
                ) from error
            descriptor = await self.wave.describe(
                cast(dict[str, str], published["snapshot"])["address"]
            )
            return captured.namespace, descriptor

    @staticmethod
    def _check_owner(snapshot: SnapshotDescriptor, namespace: Namespace) -> None:
        if (
            not isinstance(snapshot.owner, CognitionOwner)
            or snapshot.owner.namespace_id != namespace.id
        ):
            raise NousError("cognition.namespace_mismatch", "Snapshot belongs to another namespace")

    async def refresh(
        self,
        namespace: str | None = None,
        *,
        mode: Mode | None = None,
        include_review: bool | None = None,
        include_exploration: bool | None = None,
    ) -> dict[str, object]:
        """Ensure a retrieval snapshot for the selected namespace.

        A matching current snapshot is reused; otherwise the service captures and
        publishes a new one. The optional mode and inclusion flags override the
        operation configuration.

        Returns:
            A descriptor containing the namespace, snapshot, covered content generation,
            profile and metadata. Compare the generation with current content.

        Raises:
            NousError: ``cognition.namespace_mismatch`` for another bound namespace;
                ``cognition.space_required`` when the selected mode requires an
                unconfigured embedding space; or ``cognition.projection_failed`` when
                Wave cannot build the view.
        """
        async with self.execution.operation("cognition.refresh"):
            selected = self._options(namespace, mode, include_review, include_exploration)
            ns, snapshot = await self._current_view(selected)
            return {
                "namespace": ns.model_dump(mode="json"),
                "snapshot": snapshot.ref.model_dump(mode="json"),
                "covered_generation": snapshot.captured_generation,
                "profile": snapshot.profile_signature,
                "metadata": snapshot.metadata,
            }

    async def query(self, request: CognitiveQuery) -> QueryResult:
        """Query a current or explicitly selected snapshot and return grouped delivery.

        Without ``request.snapshot``, the service reuses a matching current view or
        builds one. An explicit snapshot is checked against the selected namespace.

        Returns:
            Grouped results and actual delivered selections, with the snapshot and
            generation context required to record query feedback.

        Raises:
            NousError: ``cognition.namespace_mismatch`` for another bound namespace or
                snapshot owner; ``cognition.space_required`` when the selected mode
                requires an unconfigured embedding space; or
                ``cognition.projection_failed`` if Wave cannot build the current view.
        """
        async with self.execution.operation("cognition.query") as operation:
            selected = self._options(
                request.namespace, request.mode, request.include_review, request.include_exploration
            )
            if request.snapshot:
                async with self.database.read() as tx:
                    ns = await self.scope.get_in(tx, selected.namespace)
                snapshot = await self.wave.describe(request.snapshot)
                self._check_owner(snapshot, ns)
            else:
                ns, snapshot = await self._current_view(selected)
            settings = self.execution.snapshot.typed_values()
            result = await self.wave.query(
                WaveQuery(
                    snapshot=snapshot.ref.address,
                    text=request.text,
                    mode=selected.mode,
                    top_k=request.top_k or settings.wave.query.top_k,
                    grouping="document",
                    passages_per_group=settings.cognition.search.passages_per_record,
                    include_trace=request.explain,
                    context=request.context,
                    core_concepts=request.core_concepts,
                    cues=request.cues,
                    candidate_filter=request.filters,
                )
            )
            groups: list[SearchGroup] = []
            delivery: list[DeliveredSelection] = []
            for raw in cast(list[dict[str, object]], result["groups"]):
                revision = RecordRevisionRef.model_validate(raw["revision_ref"])
                passages: list[SearchPassage] = []
                raw_passages = cast(list[dict[str, object]], raw["passages"])
                for hit in raw_passages:
                    fields = {
                        "candidate_ref",
                        "content",
                        "origin_contents",
                        "candidate_rank",
                        "text",
                        "snippet",
                        "quality_score",
                        "ranking_score",
                        "score_kind",
                        "channels",
                        "channel_ranks",
                        "channel_scores",
                    }
                    passage = SearchPassage.model_validate(
                        {
                            **{key: hit.get(key) for key in fields},
                            "channel_ranks": hit.get("channel_ranks") or {},
                            "channel_scores": hit.get("channel_scores") or {},
                            "evidence_roots": hit["sources"],
                            "concept_refs": hit["concepts"],
                            "details": {
                                key: value
                                for key, value in hit.items()
                                if key
                                in {
                                    "trace",
                                    "chosen_path",
                                    "paths",
                                    "independent_support",
                                    "folded_refs",
                                    "counterevidence",
                                    "counterevidence_refs",
                                }
                            },
                        }
                    )
                    passages.append(passage)
                    delivery.append(
                        DeliveredSelection(
                            revision_ref=revision,
                            candidate_ref=passage.candidate_ref,
                            content=passage.content,
                            group_rank=cast(int, raw["group_rank"]),
                            candidate_rank=passage.candidate_rank,
                        )
                    )
                metadata = cast(dict[str, object], raw_passages[0]["metadata"])
                groups.append(
                    SearchGroup(
                        group_key=cast(str, raw["group_key"]),
                        revision_ref=revision,
                        title=cast(str | None, raw["title"]),
                        group_rank=cast(int, raw["group_rank"]),
                        representative=CandidateRef.model_validate(raw["representative"]),
                        passages=tuple(passages),
                        sources=TypeAdapter(tuple[SourceBinding, ...]).validate_python(
                            metadata["sources"]
                        ),
                        needs_review=cast(bool, metadata["needs_review"]),
                    )
                )
            summary = cast(dict[str, object], result["context_summary"])
            context = QueryContext(
                request_text=request.text,
                namespace_id=ns.id,
                snapshot_ref=snapshot.ref,
                captured_generation=snapshot.captured_generation,
                profile_signature=snapshot.profile_signature,
                space_signature=snapshot.space_signature,
                mode=selected.mode,
                selected_context=TypeAdapter(tuple[SelectedContext, ...]).validate_python(
                    summary.get("selected_context", [])
                ),
                omitted_context_count=cast(
                    int, summary.get("omitted_context_count", len(request.context))
                ),
                effective_query_options=TypeAdapter(dict[str, JsonValue]).validate_python(
                    summary["effective_query_options"]
                ),
                filters=request.filters,
                core_concept_refs=tuple(cast(list[str], summary["core_keys"])),
                temporary_cues=request.cues,
                purpose=request.purpose,
            )
            return QueryResult(
                context=context,
                namespace=ns,
                groups=tuple(groups),
                delivery=tuple(delivery),
                coverage=SearchCoverage.model_validate(result["coverage"]),
                operation_context=QueryOperationContext(
                    operation_id=operation.diagnostics.operation_id,
                    root_name=operation.diagnostics.name,
                    stage=operation.stage,
                ),
                score_kind=cast(str, result["score_kind"]),
                used_lanes=tuple(cast(list[str], result["used_lanes"])),
                skipped_lanes=TypeAdapter(tuple[dict[str, JsonValue], ...]).validate_python(
                    result["skipped_lanes"]
                ),
                algorithm_versions=cast(dict[str, str], result["algorithm_versions"]),
                needs_review_count=cast(int, snapshot.metadata.get("needs_review_count", 0)),
                thinking=TypeAdapter(dict[str, JsonValue] | None).validate_python(
                    result["thinking"]
                ),
                diagnostics=TypeAdapter(dict[str, JsonValue] | None).validate_python(
                    result.get("diagnostics")
                ),
            )

    async def status(self) -> dict[str, object]:
        """Report content and published generations without rebuilding or calling models."""
        async with self.execution.operation("cognition.retrieval_status"):
            failure = None
            try:
                selected = self._options(None, None, None, None)
            except NousError as error:
                failure = error.to_dict()
                settings = self.execution.snapshot.typed_values().cognition
                selected = SearchOptions(
                    settings.default_namespace,
                    settings.search.default_mode
                    if settings.search.default_mode != "auto"
                    else "hybrid",
                    settings.search.space,
                    settings.search.include_review,
                    settings.search.include_exploration,
                    settings.search.local_candidate_max_bytes,
                )
            profile = (
                None
                if failure
                else self.wave.profile(
                    selected.space,
                    {
                        "include_review": selected.include_review,
                        "include_exploration": selected.include_exploration,
                        "local_candidate_max_bytes": selected.candidate_limit,
                    },
                )
            )
            namespaces = await self.scope.list()
            if self.namespace is not None:
                namespaces = tuple(ns for ns in namespaces if ns.id == self.namespace.id)
            states: list[dict[str, object]] = []
            async with self.database.read() as tx:
                for ns in namespaces:
                    heads = list(
                        (
                            await tx.connection.scalars(
                                sa.select(schema.records.c.head_revision_id).where(
                                    schema.records.c.namespace_id == ns.id,
                                    schema.records.c.archived.is_(False),
                                    schema.records.c.recall.is_(True),
                                )
                            )
                        ).all()
                    )
                    needs_review = len(await basis.needs_review_in(tx, heads))
                    states.append(
                        {
                            "namespace": ns.name,
                            "namespace_id": ns.id,
                            "content_generation": ns.content_generation,
                            "needs_review": needs_review,
                        }
                    )
            for state in states:
                current = (
                    await self.wave.current(f"cognition:{state['namespace_id']}:{profile}")
                    if profile is not None
                    else None
                )
                covered = current.captured_generation if current else None
                state["covered_generation"] = covered
                state["snapshot"] = current.ref.model_dump(mode="json") if current else None
                state["pending"] = (
                    state["content_generation"] != covered
                    if covered is not None
                    else state["content_generation"] != 0
                )
            return {
                "profile": profile,
                "mode": selected.mode,
                "namespaces": states,
                "state": "unconfigured" if failure else "ready",
                "error": failure,
            }
