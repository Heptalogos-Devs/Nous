from fastapi import APIRouter, Request
from nous_sdk.contracts import CognitiveRef, RetentionChange, RetentionChanges
from nous_sdk.contracts.retention import DecayRetention

from .security import application, context

router = APIRouter(prefix='/api/v1/namespaces/{namespace_id}/retention')


@router.post("/decay")
async def decay(request: Request, namespace_id: str,  command: DecayRetention):
    return await application(request).cognition.decay_retention(await context(request, namespace_id,  capability="manage"), command)


@router.post("/change")
async def change(request: Request, namespace_id: str,  command: RetentionChange):
    return await application(request).cognition.change_retention(await context(request, namespace_id,  capability="manage"), command)


@router.post("/change-many")
async def change_many(request: Request, namespace_id: str,  command: RetentionChanges):
    return await application(request).cognition.change_retentions(await context(request, namespace_id,  capability="manage"), command)


@router.post("/snapshot")
async def snapshot(request: Request, namespace_id: str,  reference: CognitiveRef):
    return await application(request).cognition.retention_snapshot(await context(request, namespace_id,  capability="read"), reference)
