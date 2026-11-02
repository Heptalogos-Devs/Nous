"""Retrieval tuning and profile operations use the native bounded execution owner."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.tuning import (
    ApplyRetrievalProfile,
    CreateTuning,
    RollbackRetrievalProfile,
)

from .security import application, context

router = APIRouter(prefix='/namespaces/{namespace_id}/retrieval')


@router.post("/tuning")
async def create(request: Request, namespace_id: str,  command: CreateTuning):
    return await application(request).tuning.create(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.get("/tuning")
async def tasks(
    request: Request, namespace_id: str,  limit: int = 50, cursor: str | None = None
):
    return await application(request).tuning.list(
        await context(request, namespace_id,  capability="read"),
        limit=limit,
        cursor=cursor,
    )


@router.get("/tuning/{task_id}")
async def task(request: Request, namespace_id: str,  task_id: str):
    return await application(request).tuning.get(
        await context(request, namespace_id,  capability="read"), task_id
    )


@router.get("/tuning/{task_id}/rounds")
async def rounds(
    request: Request,
    namespace_id: str,
    
    task_id: str,
    limit: int = 100,
    after: int | None = None,
):
    return await application(request).tuning.read_rounds(
        await context(request, namespace_id,  capability="read"),
        task_id,
        limit=limit,
        after=after,
    )


@router.post("/tuning/{task_id}/cancel")
async def cancel(request: Request, namespace_id: str,  task_id: str):
    return await application(request).tuning.cancel(
        await context(request, namespace_id,  capability="manage"), task_id
    )


@router.get("/profile")
async def profile(request: Request, namespace_id: str):
    return await application(request).tuning.profile(
        await context(request, namespace_id,  capability="read")
    )


@router.post("/profile/apply")
async def apply(
    request: Request, namespace_id: str,  command: ApplyRetrievalProfile
):
    return await application(request).tuning.change_profile(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.post("/profile/rollback")
async def rollback(
    request: Request, namespace_id: str,  command: RollbackRetrievalProfile
):
    return await application(request).tuning.change_profile(
        await context(request, namespace_id,  capability="manage"), command
    )
