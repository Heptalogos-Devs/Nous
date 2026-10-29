"""Read source operations through their scoped lifecycle owner."""

from fastapi import APIRouter, Query, Request

from .security import application, context

router = APIRouter()


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/sources/operation")
async def source_operation(
    request: Request,
    namespace_id: str,
    subject_id: str,
    commit_seq: int | None = None,
    source_operation_id: str | None = None,
):
    return await application(request).sources.get_operation(
        await context(request, namespace_id, subject_id, capability="read"),
        source_operation_id,
        commit_seq=commit_seq,
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/sources/operations")
async def source_operations(
    request: Request,
    namespace_id: str,
    subject_id: str,
    limit: int = Query(default=100, ge=1, le=1000),
    before_commit_seq: int | None = None,
):
    return await application(request).sources.list_operations(
        await context(request, namespace_id, subject_id, capability="read"),
        limit,
        before_commit_seq=before_commit_seq,
    )
