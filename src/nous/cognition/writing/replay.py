"""Reconstruct the current outcome of an idempotent record write."""

from __future__ import annotations

from typing import TYPE_CHECKING, cast

from nous.common.operations import OperationIssue

from .contracts import AfterWrite, CommittedRecord, OperationLink, RecordOutcome, StageOutcome

if TYPE_CHECKING:
    from nous.jobs.contracts import JobRef
    from nous.storage.database import Database

    from ..annotation.operations import AnnotationOperations
    from ..maintenance.operations import MaintenanceOperations
    from ..retrieval.operations import CognitiveRetrieval


async def replay_write(
    record: CommittedRecord,
    after: AfterWrite,
    database: Database,
    annotations: AnnotationOperations,
    maintenance: MaintenanceOperations,
    retrieval_service: CognitiveRetrieval,
    operation: OperationLink,
) -> RecordOutcome:
    """Read current maintenance facts without repeating the committed write."""
    annotation = StageOutcome(state="not_requested")
    issues: list[OperationIssue] = []
    jobs: list[JobRef] = []
    if after.annotation != "skip":
        async with database.read() as tx:
            active = await annotations.read_many_in(tx, [record.revision_ref])
        view = active.get(record.revision_ref.revision_id)
        if view is not None:
            annotation = StageOutcome(
                state="ready", ref=view.ref.identifier, generation=view.generation
            )
        else:
            job = await maintenance.record_annotation_job(record.revision_ref)
            if job is not None:
                jobs.append(job.ref)
            if job is not None and job.status in {
                "ERROR",
                "CANCELLED",
                "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
            }:
                issue = job.error or OperationIssue(
                    "annotation", "job." + job.status.lower(), f"Job ended in {job.status}"
                )
                issues.append(issue)
                annotation = StageOutcome(state="failed", issue=issue, job=job.ref)
            elif job is not None and job.status in {"ENQUEUED", "PENDING", "DELAYED"}:
                annotation = StageOutcome(state="pending", job=job.ref)
            elif job is not None or after.annotation == "run":
                annotation = StageOutcome(state="pending")

    requested = after.retrieval == "refresh" or after.retrieval_mode is not None
    retrieval = StageOutcome(state="pending" if record.in_recall or requested else "not_requested")
    if record.in_recall or requested:
        status = await retrieval_service.status(mode=after.retrieval_mode)
        namespaces = cast(list[dict[str, object]], status["namespaces"])
        current = next(
            (value for value in namespaces if value["namespace_id"] == record.namespace.id),
            None,
        )
        if current is not None:
            covered = current["covered_generation"]
            snapshot = current["snapshot"]
            if isinstance(covered, int) and covered >= record.committed_generation:
                ref = cast(dict[str, object], snapshot) if isinstance(snapshot, dict) else {}
                if isinstance(ref.get("identifier"), str):
                    retrieval = StageOutcome(
                        state="ready", generation=covered, ref=cast(str, ref["identifier"])
                    )
        if requested:
            job = await maintenance.record_refresh_job(record)
            if job is not None:
                jobs.append(job.ref)
                if retrieval.state != "ready" and job.status in {
                    "ERROR",
                    "CANCELLED",
                    "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
                }:
                    issue = job.error or OperationIssue(
                        "retrieval",
                        "job." + job.status.lower(),
                        f"Job ended in {job.status}",
                    )
                    issues.append(issue)
                    retrieval = StageOutcome(state="failed", issue=issue, job=job.ref)
                elif retrieval.state != "ready" and job.status in {
                    "ENQUEUED",
                    "PENDING",
                    "DELAYED",
                }:
                    retrieval = StageOutcome(state="pending", job=job.ref)
    return RecordOutcome(
        record=record,
        status="partial" if issues else "success",
        annotation=annotation,
        retrieval=retrieval,
        operation=operation,
        jobs=tuple(jobs),
    )
