"""Host runtime lifecycle calls, separate from public model tools."""

import asyncio
from typing import Literal

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
from nous_sdk.contracts.context import (
    ChangeContextSession,
    ChangeResidents,
    ChangeWorkContext,
)

from .public import bound_context
from .security import application

router = APIRouter(prefix="/api/runtime/namespaces/{namespace_id}", tags=["runtime"])


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


@router.post("/sessions/{session}/read")
async def read_runtime_source(
    request: Request, namespace_id: str, session: str, command: RuntimeRead
):
    if command.binding.session != session:
        raise DomainError("runtime_session_mismatch", "Request selects another session")
    return await application(request).runtime.read(
        await bound_context(request, namespace_id, "read"), command
    )


@router.get("/sessions/{session}")
async def read_session(request: Request, namespace_id: str, session: str):
    return await application(request).runtime.read_session(
        await bound_context(request, namespace_id, "read"), session
    )


@router.post("/sessions/{session}/change")
async def change_session(
    request: Request,
    namespace_id: str,
    session: str,
    command: ChangeContextSession,
):
    if command.session_id != session:
        raise DomainError("runtime_session_mismatch", "Request selects another session")
    return await application(request).runtime.change_session(
        await bound_context(request, namespace_id, "write"), command
    )


@router.post("/sessions/{session}/close")
async def close_session(request: Request, namespace_id: str, session: str):
    return await application(request).runtime.close_session(
        await bound_context(request, namespace_id, "write"), session
    )


@router.post("/sessions/{session}/residents")
async def change_residents(
    request: Request,
    namespace_id: str,
    session: str,
    command: ChangeResidents,
):
    if command.session_id != session:
        raise DomainError("runtime_session_mismatch", "Request selects another session")
    return await application(request).runtime.change_residents(
        await bound_context(request, namespace_id, "write"), command
    )


@router.get("/work")
async def list_work(
    request: Request, namespace_id: str, state: str | None = None, limit: int = 100
):
    return await application(request).runtime.list_work(
        await bound_context(request, namespace_id, "read"), state=state, limit=limit
    )


@router.post("/work")
async def change_work(request: Request, namespace_id: str, command: ChangeWorkContext):
    return await application(request).runtime.change_work(
        await bound_context(request, namespace_id, "write"), command
    )


@router.get("/work/{work_context_id}")
async def read_work(request: Request, namespace_id: str, work_context_id: str):
    return await application(request).runtime.read_work(
        await bound_context(request, namespace_id, "read"), work_context_id
    )


@router.get("/work/{work_context_id}/history")
async def work_history(
    request: Request, namespace_id: str, work_context_id: str, limit: int = 100
):
    return await application(request).runtime.work_history(
        await bound_context(request, namespace_id, "read"), work_context_id, limit
    )


@router.get("/jobs")
async def job_page(
    request: Request,
    namespace_id: str,
    limit: int = 50,
    cursor: str | None = None,
    scope: str | None = None,
    kind: str | None = None,
    state: str | None = None,
    purpose: Literal["actor", "management"] = "actor",
):
    ctx = await bound_context(
        request, namespace_id, "manage" if purpose == "management" else "read"
    )
    page = await application(request).execution.query_jobs(
        ctx,
        limit=limit,
        cursor=cursor,
        scope=scope,
        kind=kind,
        state=state,
        purpose=purpose,
    )
    return {"items": page.items, "next_cursor": page.next_cursor}


@router.get("/jobs/{job_id}")
async def job(
    request: Request,
    namespace_id: str,
    job_id: str,
    purpose: Literal["actor", "management"] = "actor",
):
    ctx = await bound_context(
        request, namespace_id, "manage" if purpose == "management" else "read"
    )
    return {"job": await application(request).execution.status(ctx, job_id, purpose=purpose)}


@router.post("/jobs/{job_id}/{action}")
async def job_action(
    request: Request,
    namespace_id: str,
    job_id: str,
    action: Literal["cancel", "resume", "wait"],
    wait_seconds: float = 120,
    purpose: Literal["actor", "management"] = "actor",
):
    capability = "manage" if purpose == "management" else "read" if action == "wait" else "write"
    ctx = await bound_context(request, namespace_id, capability)
    execution = application(request).execution
    if action == "wait":
        async with asyncio.timeout(wait_seconds):
            return await execution.wait(ctx, job_id, purpose=purpose)
    if action == "cancel":
        await execution.cancel(ctx, job_id, purpose=purpose)
    else:
        await execution.resume(ctx, job_id, purpose=purpose)
    return {"id": job_id, "action": action}
