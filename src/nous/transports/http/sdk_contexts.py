"""Public SDK resource routes backed by the shared domain owners."""

from fastapi import APIRouter, Request
from nous_wave.contracts.context import (
    ChangeContextSession,
    ChangeResidents,
    ChangeWorkContext,
    ContextAdmissionCommand,
    ContextPrepare,
    ContextReport,
    ContextTrack,
    OpenContextSession,
    ReadContextSource,
)

from .sdk_common import OperationIdentity
from .security import application, context

router = APIRouter()


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/source")
async def context_source(
    request: Request, namespace_id: str, subject_id: str, command: ReadContextSource
):
    return await application(request).contexts.read_context_source(
        await context(request, namespace_id, subject_id, capability="write"),
        command,
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/track")
async def read_context_track(
    request: Request, namespace_id: str, subject_id: str, command: ContextTrack
):
    return await application(request).contexts.read_track(
        await context(request, namespace_id, subject_id, capability="read"),
        command,
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparations")
async def prepare_context(
    request: Request, namespace_id: str, subject_id: str, command: ContextPrepare
):
    return await application(request).contexts.prepare(
        await context(request, namespace_id, subject_id), command
    )


@router.get(
    "/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparations/{preparation_id}"
)
async def preparation(request: Request, namespace_id: str, subject_id: str, preparation_id: str):
    return await application(request).contexts.preparation(
        await context(request, namespace_id, subject_id, capability="read"), preparation_id
    )


@router.post(
    "/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparations/{preparation_id}/cancel"
)
async def cancel_preparation(
    request: Request,
    namespace_id: str,
    subject_id: str,
    preparation_id: str,
    command: OperationIdentity,
):
    return await application(request).contexts.cancel_preparation(
        await context(request, namespace_id, subject_id), preparation_id, command.operation_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/admissions")
async def admit_managed(
    request: Request, namespace_id: str, subject_id: str, command: ContextAdmissionCommand
):
    return await application(request).contexts.admit_managed(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/reports")
async def report_managed(
    request: Request, namespace_id: str, subject_id: str, command: ContextReport
):
    return await application(request).contexts.report_managed(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/sessions")
async def open_session(
    request: Request, namespace_id: str, subject_id: str, command: OpenContextSession
):
    return await application(request).contexts.open_session(
        await context(request, namespace_id, subject_id),
        command.operation_id,
        command.session_id,
        parent_session_id=command.parent_session_id,
        history_generation=command.history_generation,
    )


@router.post(
    "/namespaces/{namespace_id}/subjects/{subject_id}/contexts/sessions/{session_id}/close"
)
async def close_session(
    request: Request,
    namespace_id: str,
    subject_id: str,
    session_id: str,
    command: OperationIdentity,
):
    return await application(request).contexts.close_session(
        await context(request, namespace_id, subject_id), session_id, command.operation_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparation-jobs")
async def submit_context_preparation(
    request: Request, namespace_id: str, subject_id: str, command: ContextPrepare
):
    return await application(request).contexts.submit_prepare(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/sessions/{session_id}")
async def read_context_session(
    request: Request, namespace_id: str, subject_id: str, session_id: str
):
    return await application(request).contexts.read_session(
        await context(request, namespace_id, subject_id, capability="read"), session_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/residents")
async def change_context_residents(
    request: Request, namespace_id: str, subject_id: str, command: ChangeResidents
):
    return await application(request).contexts.change_residents(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/work")
async def change_work_context(
    request: Request, namespace_id: str, subject_id: str, command: ChangeWorkContext
):
    return await application(request).contexts.change_work(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/work/{work_context_id}")
async def read_work_context(
    request: Request, namespace_id: str, subject_id: str, work_context_id: str
):
    return await application(request).contexts.read_work(
        await context(request, namespace_id, subject_id, capability="read"), work_context_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/sessions/change")
async def change_context_session(
    request: Request, namespace_id: str, subject_id: str, command: ChangeContextSession
):
    return await application(request).contexts.change_session(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/work")
async def list_work_contexts(request: Request, namespace_id: str, subject_id: str, limit: int = 50):
    return await application(request).contexts.list_work(
        await context(request, namespace_id, subject_id, capability="read"), limit
    )


@router.get(
    "/namespaces/{namespace_id}/subjects/{subject_id}/contexts/work/{work_context_id}/history"
)
async def work_context_history(
    request: Request, namespace_id: str, subject_id: str, work_context_id: str, limit: int = 50
):
    return await application(request).contexts.work_history(
        await context(request, namespace_id, subject_id, capability="read"), work_context_id, limit
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/admissions/{admission_id}")
async def read_context_admission(
    request: Request, namespace_id: str, subject_id: str, admission_id: str
):
    return await application(request).contexts.read_admission(
        await context(request, namespace_id, subject_id, capability="read"), admission_id
    )
