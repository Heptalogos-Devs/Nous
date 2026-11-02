"""A deletion cutoff prevents old in-flight work from republishing removed derived data."""

from nous_sdk.contracts import DomainError
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

from .contracts import Request
from .schema import requests, space_retirements


async def require_live_in(view: ReadView | WriteTx, request: Request, space: str) -> None:
    cutoff = await view.connection.scalar(
        select(space_retirements.c.commit_seq).where(
            space_retirements.c.namespace_id == request.context.namespace_id,
            space_retirements.c.space_id == space,
        )
    )
    if cutoff is None:
        return
    admitted = request.admitted_commit
    if admitted is None:
        # Existing projection rows retained their original authority admission.
        root = request.job_id.removeprefix("vectors:")
        admitted = await view.connection.scalar(
            select(requests.c.commit_seq).where(
                requests.c.id == root, requests.c.namespace_id == request.context.namespace_id
            )
        )
    if admitted is None or admitted <= cutoff:
        raise DomainError(
            "vector_space_deleted", "This projection predates deletion of its vector space"
        )
