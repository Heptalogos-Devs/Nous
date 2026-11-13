"""Coordinate current projections and return the actual grouped cognitive delivery."""

from __future__ import annotations

from dataclasses import dataclass, replace
from time import perf_counter

from nous.application.contracts import CallContext, LanguageEvents, define_operation
from nous.common.errors import NousError
from nous.common.keyed_locks import KeyedLocks
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ResultRef
from nous.content.results import ResultStore
from nous.content.text import PARSER_PROFILE
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database
from nous.temporal.domains import TimeService
from nous.wave.contracts import (
    CognitionOwner,
    CorpusView,
    GroupDescriptor,
    ReusableDocument,
    SnapshotDescriptor,
)
from nous.wave.operations import WaveOperations
from nous.wave.program import resolve_program
from nous.wave.serialization import content_signature

from ..annotation.operations import AnnotationOperations
from ..contracts import Namespace, NamespaceRef, NamespaceSelector
from ..material_contracts import MaterialIndex
from ..materials import Materials
from ..scope import NamespaceOperations
from ..steward.operations import StewardOperations
from ..work.operations import WorkOperations
from .adoption import EvidenceAdoption
from .capture import CorpusCapture
from .contracts import (
    CognitiveGroupContext,
    QueryRequest,
    RefreshResult,
    RefreshRetrieval,
    RetrievalOutcome,
    SaveEvidence,
)
from .projection import ProjectionBuilder

QUERY = define_operation(
    "retrieval.query",
    request=QueryRequest,
    result=RetrievalOutcome,
    events=LanguageEvents,
    description="Search cognitive records and return exact passages",
)
SAVE_EVIDENCE = define_operation(
    "retrieval.save_evidence",
    request=SaveEvidence,
    result=ResultRef,
    description="Save a completed evidence delivery without repeating its query",
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
    program: str
    space: str | None
    include_review: bool
    include_exploration: bool
    candidate_limit: int
    include_work: bool


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
        artifacts: ArtifactStore,
        results: ResultStore,
        wave: WaveOperations,
        annotations: AnnotationOperations,
        materials: Materials,
        work: WorkOperations,
        steward: StewardOperations,
        time: TimeService,
    ):
        self.database, self.execution, self.scope = database, execution, scope
        self.artifacts, self.results, self.wave = artifacts, results, wave
        self.annotations = annotations
        self.materials, self.work, self.steward = materials, work, steward
        self.evidence_adoption = EvidenceAdoption(
            database, artifacts, results, materials, scope, wave
        )
        self.time = time
        self.capture = CorpusCapture(scope, materials)
        self.projection = ProjectionBuilder(results, materials)
        self._locks = KeyedLocks[tuple[str, str]]()

    async def resolve_options(
        self,
        namespace: NamespaceSelector,
        program: str | None,
        include_review: bool | None,
        include_exploration: bool | None,
        candidate_limit: int | None = None,
        space_override: str | None = None,
        *,
        target: NamespaceRef | None = None,
    ) -> SearchOptions:
        settings = self.execution.snapshot.values.cognition
        namespace_id = (await self.scope.resolve_for(namespace, target)).id
        wave_settings = self.execution.snapshot.values.wave
        compiled = resolve_program(wave_settings, program)
        space = space_override or settings.search.space
        if "vectors" in compiled.requirements and not space:
            if program is not None or not wave_settings.unavailable_program:
                raise NousError(
                    "cognition.space_required", "This query program requires an embedding space"
                )
            compiled = resolve_program(wave_settings, wave_settings.unavailable_program)
            if "vectors" in compiled.requirements:
                raise NousError(
                    "cognition.space_required", "The fallback program requires an embedding space"
                )
        if "vectors" not in compiled.requirements:
            space = None
        return SearchOptions(
            namespace_id,
            compiled.name,
            space,
            settings.search.include_review if include_review is None else include_review,
            settings.search.include_exploration
            if include_exploration is None
            else include_exploration,
            candidate_limit
            if candidate_limit is not None
            else settings.search.local_candidate_max_bytes,
            settings.search.include_work,
        )

    def _projection_policy(self, selected: SearchOptions) -> dict[str, object]:
        material_contract = content_signature(
            {
                "material_index_schema": MaterialIndex.model_json_schema(),
                "group_context_schema": CognitiveGroupContext.model_json_schema(),
                "material_schemas": self.materials.describe(),
                "searchable_kinds": self.materials.searchable_kinds(),
            }
        )
        return {
            "include_review": selected.include_review,
            "include_exploration": selected.include_exploration,
            "local_candidate_max_bytes": selected.candidate_limit,
            "include_work": selected.include_work,
            "material_contract": material_contract,
        }

    async def current_view(
        self,
        selected: SearchOptions,
        *,
        build_config: dict[str, object] | None = None,
    ) -> tuple[Namespace, SnapshotDescriptor, int]:
        policy = self._projection_policy(selected)
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
            self.check_owner(existing, ns)
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
                self.check_owner(existing, ns)
                return ns, existing, ns.content_generation
            captured_at = perf_counter()
            async with self.database.read() as tx:
                directory = await self.capture.directory_in(
                    tx,
                    ns.id,
                    include_review=selected.include_review,
                    include_exploration=selected.include_exploration,
                    include_work=selected.include_work,
                )
                eligible = directory.entries
                stamps: dict[str, str] = {}
                for key, entry in eligible.items():
                    material = entry.view
                    stamps[key] = content_signature(
                        {
                            "revision": material.ref.model_dump(mode="json"),
                            "body": material.body.model_dump(mode="json")
                            if material.body
                            else None,
                            "body_digest": entry.body_digest,
                            "material_contract": policy["material_contract"],
                            "title": material.title,
                            "attributes": material.attributes.model_dump(mode="json"),
                            "times": material.times.model_dump(mode="json")
                            if material.times
                            else None,
                            "index": entry.index.model_dump(mode="json"),
                            "relations": [
                                item.model_dump(mode="json") for item in material.relations
                            ],
                            "parser": PARSER_PROFILE,
                            "candidate_limit": selected.candidate_limit,
                        }
                    )
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
            for key in eligible:
                cached = reusable.get(stamps[key])
                if cached is not None:
                    fragment, group = cached
                else:
                    relevant: set[str] = set()
                    pending = [key]
                    while pending:
                        current = pending.pop()
                        if current in relevant:
                            continue
                        relevant.add(current)
                        for relation in captured.materials[current].index.support_relations:
                            target = self.materials.key(relation.target)
                            pending.append(target)
                            target_body = captured.materials[target].view.body
                            pending.extend(
                                self.materials.key(part.artifact)
                                for part in relation.evidence
                                if target_body is None or part.artifact != target_body.artifact
                            )
                    subset = replace(
                        captured,
                        materials={item: captured.materials[item] for item in relevant},
                        current=(key,),
                    )
                    fragment = await self.projection.build(
                        subset, collection, profile, candidate_limit=selected.candidate_limit
                    )
                    group = fragment.groups[0]
                fragments.append((key, stamps[key], fragment, group))
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
    def check_owner(snapshot: SnapshotDescriptor, namespace: Namespace) -> None:
        if (
            not isinstance(snapshot.owner, CognitionOwner)
            or snapshot.owner.namespace_id != namespace.id
        ):
            raise NousError("cognition.namespace_mismatch", "Snapshot belongs to another namespace")

    async def refresh(
        self,
        namespace: NamespaceSelector = None,
        *,
        include_review: bool | None = None,
        include_exploration: bool | None = None,
    ) -> RefreshResult:
        """Publish or reuse the current view for the configured query program."""
        async with self.execution.operation("cognition.refresh"):
            selected = await self.resolve_options(
                namespace, None, include_review, include_exploration
            )
            ns, snapshot, current_generation = await self.current_view(selected)
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
            include_review=request.include_review,
            include_exploration=request.include_exploration,
        )

    async def handle_query(self, request: QueryRequest, call: CallContext) -> RetrievalOutcome:
        from .query import QueryExecutor

        return await QueryExecutor(self).run(request, call)

    async def save_evidence(self, request: SaveEvidence, call: CallContext) -> ResultRef:
        return await self.evidence_adoption.save(
            request.evidence, call, retain_snapshot=request.retain_snapshot
        )

    async def status(self) -> dict[str, object]:
        """Report published retrieval views without building or calling models."""
        async with self.execution.operation("cognition.retrieval_status"):
            failure = None
            selected = None
            try:
                selected = await self.resolve_options(None, None, None, None)
            except NousError as error:
                failure = error.to_dict()
            profile = (
                self.wave.profile(selected.space, self._projection_policy(selected))
                if selected
                else None
            )
            states: list[dict[str, object]] = []
            async with self.database.read() as tx:
                namespaces = await self.scope.list_in(tx)
                for ns in namespaces:
                    directory = await self.capture.directory_in(
                        tx,
                        ns.id,
                        include_review=True,
                        include_exploration=True,
                        include_work=self.execution.snapshot.values.cognition.search.include_work,
                    )
                    states.append(
                        {
                            "namespace": ns.name,
                            "namespace_id": ns.id,
                            "content_generation": ns.content_generation,
                            "needs_review": directory.needs_review_count,
                        }
                    )
            for state in states:
                current = (
                    await self.wave.current(f"cognition:{state['namespace_id']}:{profile}")
                    if profile
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
                "program": selected.program if selected else None,
                "namespaces": states,
                "state": "unconfigured" if failure else "ready",
                "error": failure,
            }
