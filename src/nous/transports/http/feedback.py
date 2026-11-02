from fastapi import APIRouter, Request
from nous_sdk.contracts.feedback import (
    EnqueueFeedback,
    FeedbackQuery,
    RetryFeedback,
    RollbackFeedback,
)

from .security import application, context

router = APIRouter(prefix='/api/v1/namespaces/{namespace_id}/feedback')


@router.post("")
async def enqueue(request: Request, namespace_id: str,  command: EnqueueFeedback):
    return await application(request).feedback.enqueue(
        await context(request, namespace_id), command
    )


@router.post("/query")
async def query(request: Request, namespace_id: str,  command: FeedbackQuery):
    return await application(request).feedback.list(
        await context(request, namespace_id,  capability="read"), command
    )


@router.post("/rollback")
async def rollback(request: Request, namespace_id: str,  command: RollbackFeedback):
    return await application(request).feedback.rollback(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.post("/retry")
async def retry(request: Request, namespace_id: str,  command: RetryFeedback):
    return await application(request).feedback.retry(await context(request, namespace_id), command)


@router.get("/{task_id}")
async def get(request: Request, namespace_id: str,  task_id: str):
    return await application(request).feedback.get(
        await context(request, namespace_id,  capability="read"), task_id
    )
