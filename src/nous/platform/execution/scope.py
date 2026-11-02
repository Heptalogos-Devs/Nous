"""Job identity admission is shared by execution and transactional retry producers."""

from typing import Literal

import sqlalchemy as sa
from nous_sdk.contracts import CallContext, DomainError

from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.schema import job_links

JobPurpose = Literal["actor", "management"]


def require_job_purpose(ctx: CallContext, purpose: JobPurpose) -> None:
    if purpose == "management" and ctx.source_groups():
        raise DomainError(
            "job_management_scope_forbidden",
            "Managing another caller's jobs requires the complete target scope",
        )


async def require_job_in(
    view: ReadView | WriteTx, ctx: CallContext, workflow_id: str, *, purpose: JobPurpose = "actor"
) -> None:
    require_job_purpose(ctx, purpose)
    statement = sa.select(job_links.c.workflow_id).where(
        job_links.c.workflow_id == workflow_id,
        job_links.c.namespace_id == ctx.namespace_id,
    )
    if purpose == "actor":
        statement = statement.where(job_links.c.caller_id == ctx.caller_id)
    if ctx.namespace_id is not None:
        statement = statement.where(job_links.c.namespace_id == ctx.namespace_id)
    if await view.connection.scalar(statement) is None:
        raise DomainError("job_not_found", "Job does not exist in this namespace and subject scope")
