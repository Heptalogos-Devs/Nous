"""Job identity admission is shared by execution and transactional retry producers."""

import sqlalchemy as sa
from nous_wave.contracts import CallContext

from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.schema import job_links


async def require_job_in(view: ReadView | WriteTx, ctx: CallContext, workflow_id: str) -> None:
    statement = sa.select(job_links.c.workflow_id).where(
        job_links.c.workflow_id == workflow_id,
        job_links.c.caller_id == ctx.caller_id,
        job_links.c.namespace_id == ctx.namespace_id,
    )
    if ctx.subject_id is not None:
        statement = statement.where(job_links.c.subject_id == ctx.subject_id)
    if await view.connection.scalar(statement) is None:
        raise LookupError("Job does not exist in this caller's namespace and subject")
