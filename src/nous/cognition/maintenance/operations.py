"""Prepare and execute cognitive maintenance independently of its scheduler."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, cast
from uuid import NAMESPACE_URL, uuid4, uuid5

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.operations import OperationIssue, record_completed
from nous.content.contracts import ResultRef
from nous.jobs.contracts import JobRef, JobView
from nous.models.embeddings import resolved_space
from nous.models.generation import GenerationOperations
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, WriteTx
from nous.wave.snapshots import build_profile

from ..annotation.contracts import AnnotationPlan, AnnotationRequest
from ..annotation.operations import AnnotationOperations
from ..concept_contracts import AnnotationView
from ..contracts import NamespaceRef, RecordRevisionRef
from ..retrieval.operations import CognitiveRetrieval
from ..scope import NamespaceOperations
from ..writing.contracts import AfterWrite, CommittedRecord
from ..writing.reading import RecordReader, Target
from .contracts import MaintenanceOutcome, MaintenancePlan, RefreshRequest

if TYPE_CHECKING:
    from nous.jobs.operations import JobOperations

JsonObject = dict[str, JsonValue]


def _json(value: object) -> JsonObject:
    return TypeAdapter(JsonObject).validate_python(value)


def _job_id(*parts: object) -> str:
    return str(uuid5(NAMESPACE_URL, "nous:" + ":".join(map(str, parts))))


@dataclass(frozen=True)
class EnqueuedMaintenance:
    """Actual work registered in the record's transaction."""

    annotation: JobRef | None = None
    retrieval: JobRef | None = None

    @property
    def jobs(self) -> tuple[JobRef, ...]:
        return tuple(job for job in (self.annotation, self.retrieval) if job is not None)


class MaintenanceOperations:
    """Interpret maintenance policy and compose annotation and refresh transactions."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        namespaces: NamespaceOperations,
        records: RecordReader,
        generation: GenerationOperations,
        annotations: AnnotationOperations,
        retrieval: CognitiveRetrieval,
        jobs: JobOperations,
    ):
        self.database, self.execution, self.namespaces, self.records = (
            database,
            execution,
            namespaces,
            records,
        )
        self.generation, self.annotations, self.retrieval, self.jobs = (
            generation,
            annotations,
            retrieval,
            jobs,
        )

    async def prepare_after(
        self,
        after: AfterWrite,
        *,
        in_recall: bool = False,
        changed: bool = True,
        annotate: bool = True,
        task: str | None = None,
    ) -> MaintenancePlan:
        """Freeze only the work the current write actually requests."""
        settings = self.execution.snapshot.values.cognition
        execution = (
            after.execution if after.execution != "default" else settings.maintenance.execution
        )
        run_annotation = (
            changed
            and annotate
            and (
                after.annotation == "run"
                or (after.annotation == "default" and in_recall and settings.annotation.on_save)
            )
        )
        run_refresh = changed and (after.retrieval == "refresh" or after.retrieval_mode is not None)
        issues: list[OperationIssue] = []
        annotation = None
        if run_annotation:
            try:
                frozen = await self.generation.freeze_language_task(
                    task or settings.annotation.task, self.execution.snapshot
                )
                selected = await self.generation.load_language_task(frozen, self.execution.snapshot)
                if (
                    selected.resource is None
                    or selected.resource.manifest.contract_id != "concept-annotation"
                ):
                    raise NousError(
                        "model.task_contract_mismatch",
                        "Annotation task uses another output contract",
                    )
                annotation = AnnotationRequest(
                    computation_key=str(uuid4()),
                    task=frozen,
                    candidate_limit=settings.annotation.known_concept_limit,
                    eligibility=(
                        "current_recalled" if after.annotation == "default" else "current_any"
                    ),
                )
            except NousError as error:
                if error.code not in {
                    "model.task_unconfigured",
                    "model.task_contract_mismatch",
                    "prompt.not_found",
                    "prompt.invalid",
                }:
                    raise
                issues.append(
                    OperationIssue(
                        "annotation",
                        error.code,
                        error.message,
                        dict(error.details),
                        error.retryable,
                    )
                )
        refresh = None
        if run_refresh:
            try:
                mode = after.retrieval_mode or settings.search.default_mode
                if mode == "auto":
                    mode = "hybrid" if settings.search.space else "lexical"
                space = None if mode == "lexical" else settings.search.space
                if mode != "lexical" and space is None:
                    raise NousError(
                        "cognition.space_required",
                        "This retrieval mode requires an embedding space",
                    )
                space_definition = (
                    _json(resolved_space(self.execution.snapshot, space).model_dump(mode="json"))
                    if space
                    else None
                )
                refresh = RefreshRequest(
                    mode=mode,
                    space=space,
                    space_definition=space_definition,
                    include_review=settings.search.include_review,
                    include_exploration=settings.search.include_exploration,
                    candidate_limit=settings.search.local_candidate_max_bytes,
                    build_config=_json(build_profile(self.execution.snapshot)),
                )
            except NousError as error:
                if error.code not in {
                    "cognition.space_required",
                    "embedding.space_unknown",
                    "embedding.deployment_conflict",
                }:
                    raise
                issues.append(
                    OperationIssue(
                        "retrieval", error.code, error.message, dict(error.details), error.retryable
                    )
                )
        return MaintenancePlan(
            execution=execution,
            annotation=annotation,
            refresh=refresh,
            unavailable=tuple(issues),
            recall_precondition=(
                in_recall
                if changed
                and annotate
                and after.annotation == "default"
                and settings.annotation.on_save
                else None
            ),
        )

    async def request_annotation(
        self,
        target: Target,
        *,
        namespace: str | None = None,
        task: str | None = None,
        refresh: bool = False,
    ) -> JobRef:
        """Enqueue a new current-revision annotation without changing the document."""
        async with self.execution.operation("maintenance.annotation.request"):
            async with self.database.read() as tx:
                row, revision, ns = await self.records.resolve_in(tx, target, namespace)
                if row["head_revision_id"] != revision["id"]:
                    raise NousError(
                        "cognition.annotation_conflict",
                        "Background annotation requires the current revision",
                    )
                exact = self.records.revision_ref(row, revision)
            plan = await self.prepare_after(
                AfterWrite(
                    annotation="run",
                    retrieval="refresh" if refresh else "defer",
                    execution="background",
                ),
                in_recall=bool(row["recall"] and not row["archived"]),
                task=task,
            )
            if plan.unavailable:
                issue = plan.unavailable[0]
                raise NousError(issue.code, issue.message, details=issue.details)
            assert plan.annotation is not None
            async with self.database.write() as tx:
                row, revision, _ = await self.records.resolve_in(tx, exact, ns.id)
                if row["head_revision_id"] != revision["id"]:
                    raise NousError(
                        "cognition.annotation_conflict", "Body changed before annotation was queued"
                    )
                job = await self.enqueue_annotation_in(tx, exact, plan.annotation, plan.refresh)
            record_completed(job.workflow_id)
            return job

    async def enqueue_annotation_in(
        self,
        tx: WriteTx,
        target: RecordRevisionRef,
        request: AnnotationRequest,
        refresh: RefreshRequest | None,
        *,
        record_write: bool = False,
    ) -> JobRef:
        if target.record_ref.instance_id != self.jobs.instance_id:
            raise NousError("cognition.instance_mismatch", "Record belongs to another instance")
        job_id = (
            _job_id("record-annotation", target.record_ref.instance_id, target.revision_id)
            if record_write
            else _job_id("annotation", request.computation_key)
        )
        payload: JsonObject = {
            "job_id": job_id,
            "namespace_ref": {
                "instance_id": target.record_ref.instance_id,
                "id": target.record_ref.namespace_id,
            },
            "target_revision": target.model_dump(mode="json"),
            "request": request.model_dump(mode="json"),
            "refresh": refresh.model_dump(mode="json") if refresh else None,
            "record_write": record_write,
        }
        return await self.jobs.enqueue_in(
            tx, kind="annotation", workflow_id=job_id, payload=payload
        )

    async def enqueue_refresh_in(
        self,
        tx: WriteTx,
        namespace: NamespaceRef,
        generation: int,
        request: RefreshRequest,
        *,
        key: str,
    ) -> JobRef:
        """Register the refresh in the same transaction that changed its view."""
        if namespace.instance_id != self.jobs.instance_id:
            raise NousError("cognition.instance_mismatch", "Namespace belongs to another instance")
        job_id = _job_id("refresh", key)
        payload: JsonObject = {
            "job_id": job_id,
            "namespace_ref": namespace.model_dump(mode="json"),
            "minimum_generation": generation,
            "request": request.model_dump(mode="json"),
        }
        return await self.jobs.enqueue_in(tx, kind="refresh", workflow_id=job_id, payload=payload)

    async def enqueue_in(
        self,
        tx: WriteTx,
        record: CommittedRecord,
        plan: MaintenancePlan,
    ) -> EnqueuedMaintenance:
        """Register the plan's work as part of its record commit."""
        if plan.execution != "background" or not record.changed:
            return EnqueuedMaintenance()
        annotation = (
            await self.enqueue_annotation_in(
                tx, record.revision_ref, plan.annotation, plan.refresh, record_write=True
            )
            if plan.annotation is not None
            else None
        )
        retrieval = None
        if plan.refresh is not None:
            retrieval = await self.enqueue_refresh_in(
                tx,
                record.namespace.ref,
                record.namespace.content_generation,
                plan.refresh,
                key=f"record:{record.revision_ref.revision_id}:{record.namespace.content_generation}",
            )
        return EnqueuedMaintenance(annotation=annotation, retrieval=retrieval)

    async def record_annotation_job(self, target: RecordRevisionRef) -> JobView | None:
        """Read the annotation job registered by this exact record write, when present."""
        ref = JobRef(
            instance_id=target.record_ref.instance_id,
            workflow_id=_job_id(
                "record-annotation", target.record_ref.instance_id, target.revision_id
            ),
        )
        try:
            job = await self.jobs.read(ref)
        except NousError as error:
            if error.code == "job.not_found":
                return None
            raise
        if not job.input.get("record_write") or job.input.get("target_revision") != (
            target.model_dump(mode="json")
        ):
            raise NousError("job.invalid_payload", "Record annotation job has a different target")
        return job

    async def record_refresh_job(self, record: CommittedRecord) -> JobView | None:
        """Read the refresh registered by this exact write, when present."""
        key = f"record:{record.revision_ref.revision_id}:{record.committed_generation}"
        ref = JobRef(
            instance_id=record.ref.instance_id,
            workflow_id=_job_id("refresh", key),
        )
        try:
            return await self.jobs.read(ref)
        except NousError as error:
            if error.code != "job.not_found":
                raise
            return None

    async def prepare_annotation(
        self, target: RecordRevisionRef, request: AnnotationRequest
    ) -> AnnotationPlan:
        return await self.annotations.prepare_plan(target, request)

    async def generate_annotation(self, plan: AnnotationPlan) -> ResultRef:
        # A formal proposal reference is the only durable output of this stage.
        return (await self.annotations.produce_proposal(plan)).result_ref

    async def adopt_annotation(
        self,
        plan: AnnotationPlan,
        proposal: ResultRef,
        refresh: RefreshRequest | None,
    ) -> MaintenanceOutcome:
        """Commit adoption and its promised refresh together, preserving superseded output."""
        async with self.execution.operation("maintenance.annotation.adopt"):
            prepared = await self.annotations.prepare_adoption(
                plan.target,
                proposal,
                namespace=plan.target.record_ref.namespace_id,
                require_recall=plan.eligibility == "current_recalled",
                activate_for_current=plan.eligibility != "historical",
            )
            related: list[JobRef] = []
            async with self.database.write() as tx:
                committed = await self.annotations.adopt_in(tx, prepared)
                if committed.current_view_changed and refresh is not None:
                    related.append(
                        await self.enqueue_refresh_in(
                            tx,
                            committed.namespace,
                            committed.generation,
                            refresh,
                            key=f"annotation:{plan.computation_key}",
                        )
                    )
            if refresh is not None and committed.conflict is None and not related:
                ref = JobRef(
                    instance_id=plan.target.record_ref.instance_id,
                    workflow_id=_job_id("refresh", f"annotation:{plan.computation_key}"),
                )
                try:
                    await self.jobs.read(ref)
                except NousError as error:
                    if error.code != "job.not_found":
                        raise
                else:
                    related.append(ref)
            return MaintenanceOutcome(
                state="superseded" if committed.conflict else "ready",
                result_refs=(committed.proposal.identifier, committed.annotation.identifier),
                proposal_result=committed.proposal,
                annotation_ref=committed.annotation,
                activation_state="inactive" if committed.activation == "inactive" else "active",
                related_jobs=tuple(related),
                covered_generation=committed.generation,
            )

    async def annotate_inline(
        self, record: CommittedRecord, request: AnnotationRequest
    ) -> AnnotationView:
        """Execute the same frozen plan directly after an inline save."""
        plan = await self.prepare_annotation(record.revision_ref, request)
        proposal = await self.annotations.produce_proposal(plan)
        prepared = await self.annotations.prepare_adoption(
            plan.target,
            proposal.result_ref,
            namespace=plan.target.record_ref.namespace_id,
            require_recall=plan.eligibility == "current_recalled",
            activate_for_current=plan.eligibility != "historical",
        )
        async with self.database.write() as tx:
            committed = await self.annotations.adopt_in(tx, prepared)
            if committed.conflict is not None:
                issue = committed.conflict
                raise NousError(issue.code, issue.message, details=issue.details)
        return await self.annotations.read(committed.annotation, namespace=committed.namespace.id)

    async def refresh(
        self,
        namespace: NamespaceRef,
        minimum_generation: int,
        request: RefreshRequest,
        *,
        build_key: str,
    ) -> MaintenanceOutcome:
        async with self.execution.operation("maintenance.refresh"):
            await self.namespaces.resolve(namespace)
            if (
                request.space
                and resolved_space(self.execution.snapshot, request.space).model_dump(mode="json")
                != request.space_definition
            ):
                raise NousError(
                    "cognition.maintenance_resource_changed",
                    "Frozen retrieval profile is no longer available",
                )
            published = await self.retrieval.refresh(
                namespace.id,
                mode=request.mode,
                include_review=request.include_review,
                include_exploration=request.include_exploration,
                build_key=build_key,
                build_config=cast(dict[str, object], request.build_config),
                candidate_limit=request.candidate_limit,
                space_override=request.space,
                minimum_generation=minimum_generation,
            )
            covered = cast(int, published["covered_generation"])
            return MaintenanceOutcome(
                state="ready" if published["current"] else "published_not_current",
                result_refs=(cast(dict[str, str], published["snapshot"])["identifier"],),
                captured_generation=covered,
                covered_generation=covered,
            )
