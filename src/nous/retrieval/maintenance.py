"""Admit one fixed index publication for the namespace changed by a transaction."""

from nous_sdk.contracts import CallContext, DomainError

from nous.platform.execution import JobSpec
from nous.platform.persistence import WriteTx


async def enqueue_index_in(tx: WriteTx, ctx: CallContext) -> None:
    if ctx.namespace_id != tx.ctx.namespace_id:
        raise DomainError(
            "index_scope_invalid", "Index maintenance belongs to the changed namespace"
        )
    if tx.authority.execution is None:
        return
    identity = f"retrieval-index:{ctx.namespace_id}:{tx.commit_seq}"
    if identity in tx.jobs:
        return
    await tx.enqueue(
        JobSpec(
            workflow_name="nous.retrieval.index",
            queue_name="default",
            workflow_id=identity,
            scope=ctx.namespace_id,
            kwargs={"namespace_id": ctx.namespace_id, "through_commit_seq": tx.commit_seq},
        )
    )
