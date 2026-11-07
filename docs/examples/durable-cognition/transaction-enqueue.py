"""Compose annotation adoption and refresh in one caller-owned transaction."""

from nous.cognition.annotation.contracts import AnnotationCommit
from nous.cognition.annotation.operations import AnnotationOperations, PreparedAnnotationAdoption
from nous.cognition.maintenance.contracts import RefreshRequest
from nous.cognition.maintenance.operations import MaintenanceOperations
from nous.common.errors import NousError
from nous.jobs import JobRef
from nous.storage.database import Database


async def adopt_with_refresh(
    database: Database,
    annotations: AnnotationOperations,
    maintenance: MaintenanceOperations,
    prepared: PreparedAnnotationAdoption,
    refresh: RefreshRequest,
    *,
    key: str,
) -> tuple[AnnotationCommit, JobRef | None]:
    async with database.write() as tx:
        committed = await annotations.adopt_in(tx, prepared)
        if committed.conflict is not None:
            issue = committed.conflict
            raise NousError(issue.code, issue.message, details=issue.details)
        job = None
        if committed.current_view_changed:
            job = await maintenance.enqueue_refresh_in(
                tx, committed.namespace, committed.generation, refresh, key=key
            )
    return committed, job
