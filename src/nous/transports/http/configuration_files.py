"""Complete configuration documents use the configuration owner's file operations."""

from typing import Literal

from fastapi import APIRouter, Request
from nous.platform.call_context import CallContext
from pydantic import BaseModel

from .security import application, authorize, context

router = APIRouter(prefix="/api/manage/configuration")


class DocumentChange(BaseModel):
    content: str


class ReloadDocument(BaseModel):
    scope: Literal["instance", "namespace"] = "namespace"


async def file_context(
    request: Request,
    scope: Literal["instance", "namespace"],
    namespace_id: str | None,
    content: str | None = None,
) -> CallContext:
    configuration = application(request).configuration
    principal = await authorize(request, capability="manage")
    if scope == "instance":
        principal.require_instance_management()
        return await context(request, "instance", capability="manage")
    if namespace_id is None:
        raise ValueError("Namespace configuration requires namespace_id")
    ctx = await context(request, namespace_id, capability="manage")
    values = (
        configuration.parse_document(content)
        if content is not None
        else await configuration.load(ctx, scope)
    )
    if any(configuration.entry(key).instance_management for key in values):
        principal.require_instance_management()
    return ctx


@router.get("/files/current")
async def export_file(
    request: Request,
    scope: Literal["instance", "namespace"] = "namespace",
    namespace_id: str | None = None,
):
    ctx = await file_context(request, scope, namespace_id)
    return {"scope": scope, "content": await application(request).configuration.export(ctx, scope)}


@router.put("/files/current")
async def replace_file(
    request: Request,
    command: DocumentChange,
    scope: Literal["instance", "namespace"] = "namespace",
    namespace_id: str | None = None,
):
    ctx = await file_context(request, scope, namespace_id, command.content)
    return await application(request).configuration.replace(
        ctx, command.content, scope
    )


@router.post("/reload")
async def reload_file(request: Request, command: ReloadDocument, namespace_id: str | None = None):
    ctx = await file_context(request, command.scope, namespace_id)
    return await application(request).configuration.reload(ctx, command.scope)
