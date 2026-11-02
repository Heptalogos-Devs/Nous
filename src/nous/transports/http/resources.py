"""Registered external sources keep versioned originals without creating subject contact."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.resources import ChangeResource, ReadResource, RegisterResource

from .security import application, context

router = APIRouter(prefix='/api/v1/namespaces/{namespace_id}/resources')


@router.post("")
async def register(request: Request, namespace_id: str,  command: RegisterResource):
    return await application(request).resources.register(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.get("")
async def list_resources(
    request: Request,
    namespace_id: str,
    
    cursor: str | None = None,
    limit: int = 100,
):
    return await application(request).resources.list(
        await context(request, namespace_id,  capability="read"),
        cursor=cursor,
        limit=limit,
    )


@router.post("/read")
async def read_resource(
    request: Request, namespace_id: str,  command: ReadResource
):
    return await application(request).resources.read(
        await context(request, namespace_id,  capability="read"), command
    )


@router.post("/change")
async def change_resource(
    request: Request, namespace_id: str,  command: ChangeResource
):
    return await application(request).resources.change(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.get("/{resource_id}")
async def get_resource(
    request: Request,
    namespace_id: str,
    
    resource_id: str,
    revision: int | None = None,
):
    return await application(request).resources.get(
        await context(request, namespace_id,  capability="read"), resource_id, revision
    )
