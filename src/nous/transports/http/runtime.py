"""Host runtime lifecycle calls, separate from public model tools."""

from fastapi import APIRouter, Request
from nous_sdk.contracts import DomainError
from nous_sdk.contracts.runtime import (
    RuntimeComplete,
    RuntimeOpen,
    RuntimePrepare,
    RuntimeRead,
    RuntimeSent,
    RuntimeTool,
)

from .public import bound_context
from .security import application

router = APIRouter(prefix="/api/v1/host/namespaces/{namespace_id}", tags=["host"])


@router.post("/sessions")
async def open_session(request: Request, namespace_id: str, command: RuntimeOpen):
    return await application(request).runtime.open(
        await bound_context(request, namespace_id, "write"), command
    )


@router.post("/sessions/{session}/model-input")
async def before_model(request: Request, namespace_id: str, session: str, command: RuntimePrepare):
    if command.binding.session != session:
        raise DomainError("runtime_session_mismatch", "Request selects another session")
    return await application(request).runtime.prepare(
        await bound_context(request, namespace_id, "write"), command
    )


@router.post("/sessions/{session}/model-sent")
async def model_sent(request: Request, namespace_id: str, session: str, command: RuntimeSent):
    return await application(request).runtime.sent(
        await bound_context(request, namespace_id, "write"), command
    )


@router.post("/sessions/{session}/model-results")
async def after_model(request: Request, namespace_id: str, session: str, command: RuntimeComplete):
    return await application(request).runtime.complete(
        await bound_context(request, namespace_id, "write"), command
    )


@router.post("/sessions/{session}/events")
async def tool_event(request: Request, namespace_id: str, session: str, command: RuntimeTool):
    if command.binding.session != session:
        raise DomainError("runtime_session_mismatch", "Request selects another session")
    return await application(request).runtime.capture_tool(
        await bound_context(request, namespace_id, "write"), command
    )

@router.post('/sessions/{session}/read')
async def read_runtime_source(request: Request,namespace_id: str,session: str,command: RuntimeRead):
    if command.binding.session!=session:raise DomainError('runtime_session_mismatch','Request selects another session')
    return await application(request).runtime.read(await bound_context(request,namespace_id,'read'),command)
