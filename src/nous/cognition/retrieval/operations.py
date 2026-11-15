"""Coordinate current projections and return the actual grouped cognitive delivery."""

from __future__ import annotations

from dataclasses import dataclass
from time import perf_counter

from nous.application.contracts import CallContext, LanguageEvents, define_operation
from nous.common.errors import NousError
from nous.common.keyed_locks import KeyedLocks
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ResultRef
from nous.content.results import ResultStore
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database
from nous.wave.contracts import (
    CognitionOwner,
    SnapshotDescriptor,
)
from nous.wave.operations import WaveOperations
from nous.wave.program import ProgramSelection, select_program

from ..annotation.operations import AnnotationOperations
from ..contracts import Namespace, NamespaceRef, NamespaceSelector
from ..materials import Materials
from ..scope import NamespaceOperations
from ..work.reading import WorkReader
from ..writing.reading import RecordReader
from .capture import CorpusCapture
from .contracts import (
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
    selection: ProgramSelection
    space: str | None
    include_review: bool
    include_exploration: bool
    candidate_limit: int
    include_work: bool


class CognitiveRetrieval:
    """Capture and publish the current retrieval view for a namespace."""

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
        *,
        record_reader: RecordReader,
        work_reader: WorkReader,
    ):
        self.database, self.execution, self.scope = database, execution, scope
        self.wave, self.materials = wave, materials
        self.capture = CorpusCapture(
            scope, materials, artifacts, results, record_reader, work_reader, annotations
        )
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
        explore: bool = False,
    ) -> SearchOptions:
        settings = self.execution.snapshot.values.cognition
        namespace_id = (await self.scope.resolve_for(namespace, target)).id
        wave_settings = self.execution.snapshot.values.wave
        space = space_override or settings.search.space
        selected = select_program(
            wave_settings,
            program,
            explore=explore,
            capabilities=("text", "vectors", "vcp") if space else ("text",),
        )
        if "vectors" not in selected.program.requirements:
            space = None
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
            settings.search.include_work,
        )

    def _projection_policy(self, selected: SearchOptions) -> dict[str, object]:
        return {
            "include_review": selected.include_review,
            "include_exploration": selected.include_exploration,
            "local_candidate_max_bytes": selected.candidate_limit,
            "include_work": selected.include_work,
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
                captured = await self.capture.expand_in(tx, directory, tuple(directory.entries))
            capture_ms = (perf_counter() - captured_at) * 1000
            projection_at = perf_counter()
            view = await self.projection.build(
                captured,
                collection,
                profile,
                candidate_limit=selected.candidate_limit,
                needs_review_count=directory.needs_review_count,
            )

            get_logger(__name__).info(
                "cognition.capture.completed",
                extra={
                    "namespace_id": ns.id,
                    "generation": directory.namespace.content_generation,
                    "records": len(directory.entries),
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
                "program": selected.selection.program.name if selected else None,
                "namespaces": states,
                "state": "unconfigured" if failure else "ready",
                "error": failure,
            }
