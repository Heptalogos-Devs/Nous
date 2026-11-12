"""Coordinate current projections and return the actual grouped cognitive delivery."""

from __future__ import annotations

from dataclasses import dataclass, replace
from time import perf_counter
from typing import cast

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import CallContext, define_operation
from nous.common.errors import NousError
from nous.common.keyed_locks import KeyedLocks
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ExternalInput
from nous.content.results import ResultStore
from nous.content.text import PARSER_PROFILE
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database
from nous.wave.contracts import (
    CandidateFilter,
    CognitionOwner,
    ContextFragment,
    CorpusView,
    GroupDescriptor,
    ReusableDocument,
    SelectedContext,
    SnapshotDescriptor,
    WaveQuery,
)
from nous.wave.operations import WaveOperations
from nous.wave.serialization import content_signature

from .. import basis, schema
from ..annotation.operations import AnnotationOperations
from ..contracts import Namespace, NamespaceRef, NamespaceSelector, RevisionSelection
from ..scope import NamespaceOperations
from ..writing.reading import RecordReader
from .capture import CorpusCapture
from .contracts import (
    CognitiveGroup,
    CognitiveGroupContext,
    CognitiveQuery,
    Mode,
    QueryContext,
    QueryOperationContext,
    QueryResult,
    RefreshResult,
    RefreshRetrieval,
    RevisionPassage,
    SearchMode,
)
from .projection import ProjectionBuilder

QUERY = define_operation(
    "retrieval.query",
    request=CognitiveQuery,
    result=QueryResult,
    description="Search cognitive records and return exact passages",
)
REFRESH = define_operation(
    "retrieval.refresh",
    request=RefreshRetrieval,
    result=RefreshResult,
    description="Refresh the current cognitive retrieval snapshot",
)


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

    Wave owns preparation, ranking and delivery. Current queries can build and
    publish a snapshot for the selected namespace when needed.
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
    ):
        self.database, self.execution, self.scope = database, execution, scope
        self.records, self.artifacts, self.results, self.wave = records, artifacts, results, wave
        self.annotations = annotations
        self.capture = CorpusCapture(scope, records, artifacts, annotations)
        self.projection = ProjectionBuilder(results)
        self._locks = KeyedLocks[tuple[str, str]]()

    async def _options(
        self,
        namespace: NamespaceSelector,
        mode: Mode | None,
        include_review: bool | None,
        include_exploration: bool | None,
        candidate_limit: int | None = None,
        space_override: str | None = None,
        *,
        target: NamespaceRef | None = None,
    ) -> SearchOptions:
        settings = self.execution.snapshot.values.cognition
        namespace_id = (await self.scope.resolve_for(namespace, target)).id
        selected = mode or settings.search.default_mode
        if selected == "auto":
            selected = "hybrid" if settings.search.space else "lexical"
        space = None if selected == "lexical" else (space_override or settings.search.space)
        if selected != "lexical" and not space:
            raise NousError(
                "cognition.space_required", "This retrieval mode requires an embedding space"
            )
        return SearchOptions(
            namespace_id,
            selected,
            space,
            settings.search.include_review if include_review is None else include_review,
            settings.search.include_exploration
            if include_exploration is None
            else include_exploration,
            candidate_limit
            if candidate_limit is not None
            else settings.search.local_candidate_max_bytes,
        )

    async def _current_view(
        self,
        selected: SearchOptions,
        *,
        build_config: dict[str, object] | None = None,
    ) -> tuple[Namespace, SnapshotDescriptor, int]:
        policy: dict[str, object] = {
            "include_review": selected.include_review,
            "include_exploration": selected.include_exploration,
            "local_candidate_max_bytes": selected.candidate_limit,
        }
        profile = self.wave.profile(selected.space, policy, build_config=build_config)
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
            return ns, existing, ns.content_generation
        async with self._locks.hold((ns.id, profile)):
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, ns.id)
            existing = await self.wave.current(collection)
            if (
                existing is not None
                and existing.captured_generation == ns.content_generation
                and existing.profile_signature == profile
            ):
                self._check_owner(existing, ns)
                return ns, existing, ns.content_generation
            captured_at = perf_counter()
            async with self.database.read() as tx:
                directory = await self.capture.directory_in(
                    tx,
                    ns.id,
                    include_review=selected.include_review,
                    include_exploration=selected.include_exploration,
                )
                eligible = directory.entries
                stamps = {
                    key: content_signature(
                        {
                            "revision": entry.ref.model_dump(mode="json"),
                            "body_artifact": entry.revision["body_artifact_id"],
                            "body_digest": entry.body_digest,
                            "title": entry.revision["title"],
                            "purpose": entry.revision["purpose"],
                            "times": entry.revision["cognitive_times"],
                            "sources": entry.revision["source_signature"],
                            "annotation": entry.annotation_id,
                            "parser": PARSER_PROFILE,
                            "candidate_limit": selected.candidate_limit,
                        }
                    )
                    for key, entry in eligible.items()
                }
                reusable = await self.wave.match_units_in(tx, tuple(stamps.values()))
                needed = tuple(key for key, stamp in stamps.items() if stamp not in reusable)
                sources = {unit.document.key: unit for unit, _ in reusable.values()}
                pending = {
                    parent for unit in sources.values() for parent in unit.document.derived_from
                } - sources.keys()
                while pending:
                    related = await self.wave.related_units_in(tx, tuple(pending))
                    missing = pending - related.keys()
                    if missing:
                        raise NousError(
                            "wave.projection_incomplete",
                            "A reusable source node is unavailable",
                            details={"documents": sorted(missing)},
                        )
                    sources.update(related)
                    pending = {
                        parent for unit in related.values() for parent in unit.document.derived_from
                    } - sources.keys()
                captured = await self.capture.expand_in(tx, directory, needed)
            capture_ms = (perf_counter() - captured_at) * 1000
            projection_at = perf_counter()
            fragments: list[tuple[str, str, CorpusView | ReusableDocument, GroupDescriptor]] = []
            for key, entry in eligible.items():
                document_key = "revision:" + key
                cached = reusable.get(stamps[key])
                if cached is not None:
                    fragment, group = cached
                    group = group.model_copy(
                        update={"context": {**group.context, "needs_review": entry.needs_review}}
                    )
                else:
                    relevant: set[str] = set()
                    pending = [key]
                    while pending:
                        current = pending.pop()
                        if current in relevant:
                            continue
                        relevant.add(current)
                        pending.extend(
                            source.owner_revision.revision_id
                            for source in captured.records[current].sources
                            if source.usage != "inspiration" and source.owner_revision is not None
                        )
                    subset = replace(
                        captured,
                        records={item: captured.records[item] for item in relevant},
                        current=(key,),
                        annotations={key: captured.annotations[key]}
                        if key in captured.annotations
                        else {},
                        needs_review=frozenset({key} if entry.needs_review else ()),
                    )
                    fragment = await self.projection.build(
                        subset,
                        collection,
                        profile,
                        candidate_limit=selected.candidate_limit,
                    )
                    group = fragment.groups[0]
                fragments.append((document_key, stamps[key], fragment, group))
            view = self.projection.combine(
                collection,
                profile,
                directory.namespace.id,
                directory.namespace.content_generation,
                directory.needs_review_count,
                fragments,
                sources,
            )

            get_logger(__name__).info(
                "cognition.capture.completed",
                extra={
                    "namespace_id": ns.id,
                    "generation": directory.namespace.content_generation,
                    "records": len(eligible),
                    "expanded_records": len(needed),
                    "capture_ms": capture_ms,
                    "projection_ms": (perf_counter() - projection_at) * 1000,
                },
            )

            try:
                async with self.wave.build_product(
                    view,
                    self.wave.resolve_build_profile(selected.space, build_config=build_config),
                ) as prepared:
                    publish_started = perf_counter()
                    async with self.database.write() as tx:
                        descriptor = await self.wave.publish_in(tx, prepared)
                        published_generation = (
                            await self.scope.get_in(tx, ns.id)
                        ).content_generation
                        if published_generation == directory.namespace.content_generation:
                            await self.wave.promote_in(tx, descriptor.ref)
                get_logger(__name__).info(
                    "wave.publish.completed",
                    extra={
                        "snapshot": descriptor.ref.identifier,
                        "publish_ms": (perf_counter() - publish_started) * 1000,
                    },
                )
            except NousError as error:
                raise NousError(
                    "cognition.projection_failed",
                    "The current retrieval view could not be built",
                    details={
                        "namespace": ns.name,
                        "generation": directory.namespace.content_generation,
                        "cause": error.to_dict(),
                        "retry": "journal refresh",
                    },
                    retryable=error.retryable,
                    cause=error,
                ) from error
            return directory.namespace, descriptor, published_generation

    @staticmethod
    def _check_owner(snapshot: SnapshotDescriptor, namespace: Namespace) -> None:
        if (
            not isinstance(snapshot.owner, CognitionOwner)
            or snapshot.owner.namespace_id != namespace.id
        ):
            raise NousError("cognition.namespace_mismatch", "Snapshot belongs to another namespace")

    async def refresh(
        self,
        namespace: NamespaceSelector = None,
        *,
        mode: Mode | None = None,
        include_review: bool | None = None,
        include_exploration: bool | None = None,
    ) -> RefreshResult:
        """Ensure a retrieval snapshot for the selected namespace.

        A matching current snapshot is reused; otherwise the service captures and
        publishes a new one. The optional mode and inclusion flags override the
        operation configuration.

        Returns:
            A descriptor containing the namespace, snapshot, covered content generation,
            profile and metadata. Compare the generation with current content.

        Raises:
            NousError: ``cognition.space_required`` when the selected mode requires an
                unconfigured embedding space; or ``cognition.projection_failed`` when
                Wave cannot build the view.
        """
        async with self.execution.operation("cognition.refresh"):
            selected = await self._options(
                namespace,
                mode,
                include_review,
                include_exploration,
            )
            ns, snapshot, current_generation = await self._current_view(selected)
            return RefreshResult(
                namespace=ns,
                snapshot=snapshot.ref,
                captured_generation=snapshot.captured_generation,
                current_generation=current_generation,
                profile=snapshot.profile_signature,
                metadata=snapshot.metadata,
                current=current_generation == snapshot.captured_generation,
            )

    async def handle_refresh(self, request: RefreshRetrieval, _call: CallContext) -> RefreshResult:
        return await self.refresh(
            request.namespace,
            mode=request.mode,
            include_review=request.include_review,
            include_exploration=request.include_exploration,
        )

    async def handle_query(self, request: CognitiveQuery, call: CallContext) -> QueryResult:
        context, filters = request.context, request.filters
        limit = call.settings.values.content.text_read_max_bytes
        if isinstance(context, ExternalInput):
            selected = await call.inputs.read_text(context.key, context.selection, limit)
            context = TypeAdapter(tuple[ContextFragment, ...]).validate_json(selected.data)
        if isinstance(filters, ExternalInput):
            selected = await call.inputs.read_text(filters.key, filters.selection, limit)
            filters = CandidateFilter.model_validate_json(selected.data)
        return await self.query(request.model_copy(update={"context": context, "filters": filters}))

    async def query(self, request: CognitiveQuery) -> QueryResult:
        """Query a current or explicitly selected snapshot and return grouped delivery.

        Without ``request.snapshot``, the service reuses a matching current view or
        builds one. An explicit snapshot supplies its namespace unless the request
        selects one, in which case the two identities must agree.

        Returns:
            Grouped results and actual delivered selections, with the snapshot and
            generation context required to record query feedback.

        Raises:
            NousError: ``cognition.namespace_mismatch`` for a conflicting snapshot owner;
                ``cognition.space_required`` when the selected mode
                requires an unconfigured embedding space; or
                ``cognition.projection_failed`` if Wave cannot build the current view.
        """
        if isinstance(request.context, ExternalInput) or isinstance(request.filters, ExternalInput):
            raise NousError(
                "input.missing_binding", "External query inputs require an application call"
            )
        async with self.execution.operation("cognition.query") as operation:
            snapshot = await self.wave.describe(request.snapshot) if request.snapshot else None
            target = None
            if snapshot is not None:
                if not isinstance(snapshot.owner, CognitionOwner):
                    raise NousError(
                        "cognition.namespace_mismatch", "Snapshot is not a cognitive view"
                    )
                target = NamespaceRef(
                    instance_id=snapshot.ref.instance_id, id=snapshot.owner.namespace_id
                )
            selected = await self._options(
                request.namespace,
                request.mode,
                request.include_review,
                request.include_exploration,
                target=target,
            )
            if snapshot is not None:
                async with self.database.read() as tx:
                    ns = await self.scope.get_in(tx, selected.namespace)
                self._check_owner(snapshot, ns)
            else:
                ns, snapshot, _ = await self._current_view(selected)
            settings = self.execution.snapshot.values
            result = await self.wave.query(
                WaveQuery(
                    snapshot=snapshot.ref.identifier,
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
            wave_groups = result.groups
            contexts = [
                CognitiveGroupContext.model_validate(group.group.context) for group in wave_groups
            ]
            async with self.database.read() as tx:
                rows = (
                    (
                        await tx.connection.execute(
                            sa.select(
                                schema.revisions.c.id,
                                schema.revisions.c.record_id,
                                schema.revisions.c.body_artifact_id,
                            ).where(
                                schema.revisions.c.id.in_(
                                    [context.revision_ref.revision_id for context in contexts]
                                )
                            )
                        )
                    )
                    .mappings()
                    .all()
                )
            revisions = {str(item["id"]): item for item in rows}
            groups: list[CognitiveGroup] = []
            for hit, context in zip(wave_groups, contexts, strict=True):
                revision = context.revision_ref
                stored = revisions.get(revision.revision_id)
                if (
                    stored is None
                    or stored["record_id"] != revision.record_ref.id
                    or revision.record_ref.namespace_id != ns.id
                    or any(
                        fragment.content.artifact.id != stored["body_artifact_id"]
                        for passage in hit.passages
                        for fragment in passage.fragments
                    )
                ):
                    raise NousError(
                        "cognition.selection_mismatch",
                        "Delivered content does not belong to the captured revision",
                    )
                groups.append(
                    CognitiveGroup(
                        group_key=hit.group.key,
                        context=context,
                        title=hit.group.title,
                        rank=hit.rank,
                        basis=hit.basis,
                        passages=tuple(
                            RevisionPassage(
                                selection=RevisionSelection(
                                    revision=revision, selections=passage.selections
                                ),
                                delivery=passage,
                            )
                            for passage in hit.passages
                        ),
                    )
                )
            summary = result.context_summary
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
            async with self.database.read() as tx:
                current = await self.scope.get_in(tx, ns.id)
            return QueryResult(
                context=context,
                namespace=ns,
                current=current.content_generation == snapshot.captured_generation,
                groups=tuple(groups),
                coverage=result.coverage,
                operation_context=QueryOperationContext(
                    operation_id=operation.diagnostics.operation_id,
                    root_name=operation.diagnostics.name,
                    stage=operation.stage,
                ),
                score_kind=result.score_kind,
                used_lanes=result.used_lanes,
                skipped_lanes=result.skipped_lanes,
                algorithm_versions=result.algorithm_versions,
                needs_review_count=cast(int, snapshot.metadata.get("needs_review_count", 0)),
                thinking=result.thinking,
                diagnostics=result.diagnostics,
            )

    async def status(self, *, mode: Mode | None = None) -> dict[str, object]:
        """Report content and published generations without rebuilding or calling models."""
        async with self.execution.operation("cognition.retrieval_status"):
            failure = None
            try:
                selected = await self._options(None, mode, None, None)
            except NousError as error:
                failure = error.to_dict()
                settings = self.execution.snapshot.values.cognition
                selected_mode = mode or settings.search.default_mode
                if selected_mode == "auto":
                    selected_mode = "hybrid" if settings.search.space else "lexical"
                selected = SearchOptions(
                    settings.default_namespace,
                    selected_mode,
                    None if selected_mode == "lexical" else settings.search.space,
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
            states: list[dict[str, object]] = []
            async with self.database.read() as tx:
                namespaces = await self.scope.list_in(tx)
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
