"""Prepare, compute and adopt cognitive maintenance on the business loop."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, cast
from uuid import NAMESPACE_URL, uuid4, uuid5

from opentelemetry.propagate import inject
from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.jobs.contracts import JobOutcome, JobRef
from nous.models.embeddings import resolved_space
from nous.models.operations import GenerationOperations
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, WriteTx
from nous.wave.snapshots import build_profile

from ..annotations import AnnotationOperations
from ..concept_contracts import AnnotationView
from ..contracts import Namespace, NamespaceRef, RecordRevisionRef
from ..retrieval.contracts import Mode
from ..retrieval.operations import CognitiveRetrieval
from ..scope import NamespaceOperations
from ..writing.contracts import AfterWrite, CommittedRecord
from ..writing.reading import RecordReader

if TYPE_CHECKING:
    from nous.jobs.operations import JobOperations

JsonObject = dict[str, JsonValue]


def _json(value: object) -> JsonObject:
    return TypeAdapter(JsonObject).validate_python(value)


def _job_id(*parts: object) -> str:
    return str(uuid5(NAMESPACE_URL, "nous:" + ":".join(map(str, parts))))


@dataclass(frozen=True)
class EnqueuedMaintenance:
    """The actual work registered with a record commit, by business stage."""

    annotation: JobRef | None = None
    retrieval: JobRef | None = None

    @property
    def jobs(self) -> tuple[JobRef, ...]:
        return tuple(job for job in (self.annotation, self.retrieval) if job is not None)


class MaintenanceOperations:
    """Own the same bounded maintenance stages for hosts and foreground work."""

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

    async def prepare_after(self, after: AfterWrite, *, annotate: bool = True) -> JsonObject:
        """Freeze the requested maintenance policy before the owning write commits."""
        settings = self.execution.snapshot.values.cognition
        background = (
            after.execution if after.execution != "default" else settings.maintenance.execution
        ) == "background"
        if not background:
            return {"background": False}
        run_annotation = annotate and (
            after.annotation == "run"
            or after.annotation == "default"
            and settings.annotation.on_save
        )
        refresh = after.retrieval == "refresh" or after.retrieval_mode is not None
        mode = after.retrieval_mode or settings.search.default_mode
        effective_mode = (
            ("hybrid" if settings.search.space else "lexical") if mode == "auto" else mode
        )
        if not refresh:
            effective_mode = "lexical"
        space = None if effective_mode == "lexical" else settings.search.space
        if effective_mode != "lexical" and space is None:
            raise NousError(
                "cognition.space_required", "This retrieval mode requires an embedding space"
            )
        recipe: JsonObject = {
            "background": background,
            "annotation": run_annotation,
            "automatic": after.annotation != "run",
            "refresh": refresh,
            "mode": effective_mode,
            "space": space,
            "space_definition": resolved_space(self.execution.snapshot, space).model_dump(
                mode="json"
            )
            if space
            else None,
            "include_review": settings.search.include_review,
            "include_exploration": settings.search.include_exploration,
            "candidate_limit": settings.search.local_candidate_max_bytes,
            "build_config": _json(build_profile(self.execution.snapshot)),
        }
        carrier: dict[str, str] = {}
        inject(carrier)
        recipe["trace"] = _json(carrier)
        if background and run_annotation:
            frozen = await self.generation.freeze_language_task(
                settings.annotation.task, self.execution.snapshot
            )
            recipe["frozen_task"] = frozen.descriptor.model_dump(mode="json")
            recipe["candidate_limit_annotation"] = settings.annotation.known_concept_limit
        return recipe

    async def enqueue_in(
        self,
        tx: WriteTx,
        record: CommittedRecord,
        recipe: JsonObject,
        *,
        previous_generation: int,
    ) -> EnqueuedMaintenance:
        """Register work in the transaction that changed current content."""
        if not recipe["background"] or not record.changed:
            return EnqueuedMaintenance()
        namespace = record.namespace.ref
        annotation: JobRef | None = None
        retrieval: JobRef | None = None
        revision = record.revision_ref
        if recipe["annotation"] and (not recipe["automatic"] or record.in_recall):
            job_id = _job_id("annotation", revision.revision_id, recipe.get("frozen_task"))
            payload: JsonObject = {
                "job_id": job_id,
                "namespace_ref": namespace.model_dump(mode="json"),
                "target_revision": revision.model_dump(mode="json"),
                "recipe": recipe,
                "trace": recipe.get("trace", {}),
                "automatic": recipe["automatic"],
                "require_current": True,
            }
            annotation = await self.jobs.enqueue_in(
                tx, kind="annotation", workflow_id=job_id, payload=payload
            )
        if recipe["refresh"] and record.namespace.content_generation != previous_generation:
            job_id = _job_id(
                "refresh",
                namespace.id,
                record.namespace.content_generation,
                revision.revision_id,
                recipe.get("mode"),
                recipe.get("space"),
            )
            payload = {
                "job_id": job_id,
                "namespace_ref": namespace.model_dump(mode="json"),
                "minimum_generation": record.namespace.content_generation,
                "recipe": recipe,
                "trace": recipe.get("trace", {}),
            }
            retrieval = await self.jobs.enqueue_in(
                tx, kind="refresh", workflow_id=job_id, payload=payload
            )
        return EnqueuedMaintenance(annotation=annotation, retrieval=retrieval)

    async def prepare_annotation(self, payload: JsonObject) -> JsonObject:
        return await self.annotations.prepare_annotation(payload)

    async def generate_annotation(self, plan: JsonObject) -> JsonObject:
        return await self.annotations.generate_annotation(plan)

    async def adopt_annotation(self, value: JsonObject) -> JsonObject:
        async with self.execution.operation("maintenance.annotation.adopt"):
            plan = _json(value["plan"])
            recipe = _json(plan["recipe"])
            generated = _json(value["generated"])
            result = await self.annotations.save_computation(plan, generated)
            target = RecordRevisionRef.model_validate(plan["target_revision"])
            related: list[JobRef] = []

            async def on_activation(tx: WriteTx, ns: Namespace, generation: int) -> None:
                if recipe["refresh"]:
                    job_id = _job_id("annotation-refresh", plan["job_id"])
                    payload: JsonObject = {
                        "job_id": job_id,
                        "namespace_ref": ns.ref.model_dump(mode="json"),
                        "minimum_generation": generation,
                        "recipe": recipe,
                        "trace": recipe.get("trace", {}),
                    }
                    related.append(
                        await self.jobs.enqueue_in(
                            tx, kind="refresh", workflow_id=job_id, payload=payload
                        )
                    )

            try:
                adopted = await self.annotations.adopt(
                    target,
                    result.result_ref,
                    namespace=target.record_ref.namespace_id,
                    require_recall=bool(plan.get("automatic", True)),
                    on_activation=on_activation,
                )
            except NousError as error:
                if error.code != "cognition.annotation_conflict":
                    raise
                adopted = await self.annotations.adopt(
                    target,
                    result.result_ref,
                    namespace=target.record_ref.namespace_id,
                    activate=False,
                )
                return JobOutcome(
                    state="superseded",
                    result_refs=(result.result_ref.identifier, adopted.ref.identifier),
                    proposal_result=result.result_ref,
                    annotation_ref=adopted.ref,
                    activation_state="active" if adopted.active else "inactive",
                ).model_dump(mode="json")
            if recipe["refresh"] and adopted.active and not related:
                ref = JobRef(
                    instance_id=target.record_ref.instance_id,
                    workflow_id=_job_id("annotation-refresh", plan["job_id"]),
                )
                try:
                    await self.jobs.read(ref)
                except NousError as error:
                    if error.code != "job.not_found":
                        raise
                else:
                    related.append(ref)
            return JobOutcome(
                state="ready",
                result_refs=(result.result_ref.identifier, adopted.ref.identifier),
                proposal_result=result.result_ref,
                annotation_ref=adopted.ref,
                activation_state="active" if adopted.active else "inactive",
                related_jobs=tuple(related),
                covered_generation=adopted.generation,
            ).model_dump(mode="json")

    async def annotate_inline(self, record: CommittedRecord, *, automatic: bool) -> AnnotationView:
        """Execute the same frozen stages synchronously after an inline save."""
        recipe = await self.prepare_after(AfterWrite(annotation="run", execution="background"))
        recipe["refresh"] = False
        payload: JsonObject = {
            "job_id": str(uuid4()),
            "namespace_ref": record.namespace.ref.model_dump(mode="json"),
            "target_revision": record.revision_ref.model_dump(mode="json"),
            "recipe": recipe,
            "automatic": automatic,
            "require_current": True,
        }
        plan = await self.prepare_annotation(payload)
        if plan.get("state") in {"skipped", "superseded"}:
            raise NousError("cognition.annotation_conflict", "Body changed before annotation")
        generated = await self.generate_annotation(plan)
        outcome = JobOutcome.model_validate(
            await self.adopt_annotation({"plan": plan, "generated": generated})
        )
        if outcome.state != "ready":
            raise NousError(
                "cognition.annotation_conflict",
                "Body or active annotation changed during maintenance",
                details={"result_refs": list(outcome.result_refs)},
            )
        return await self.annotations.read(outcome.result_refs[-1], namespace=record.namespace.id)

    async def refresh(self, payload: JsonObject) -> JsonObject:
        async with self.execution.operation("maintenance.refresh"):
            namespace = NamespaceRef.model_validate(payload["namespace_ref"])
            await self.namespaces.resolve(namespace)
            recipe = _json(payload["recipe"])
            space = cast(str | None, recipe["space"])
            if (
                space
                and resolved_space(self.execution.snapshot, space).model_dump(mode="json")
                != recipe["space_definition"]
            ):
                raise NousError(
                    "cognition.maintenance_resource_changed",
                    "Frozen retrieval profile is no longer available",
                )
            published = await self.retrieval.refresh(
                namespace.id,
                mode=cast(Mode, recipe["mode"]),
                include_review=cast(bool, recipe["include_review"]),
                include_exploration=cast(bool, recipe["include_exploration"]),
                build_key=str(payload["job_id"]),
                build_config=cast(dict[str, object], _json(recipe["build_config"])),
                candidate_limit=cast(int, recipe["candidate_limit"]),
                space_override=space,
            )
            covered = cast(int, published["covered_generation"])
            return JobOutcome(
                state="ready" if published["current"] else "published_not_current",
                result_refs=(cast(dict[str, str], published["snapshot"])["identifier"],),
                captured_generation=covered,
                covered_generation=covered,
            ).model_dump(mode="json")
