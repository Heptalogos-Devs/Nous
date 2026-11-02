"""Authorized transport for reviewable growth and repeat policies."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.common import Operation
from nous_sdk.contracts.maintenance import (
    CaptureMaintenanceOutput,
    CreateMaintenanceSchedule,
    EditMaintenanceProposal,
    MaintenanceRun,
    MaintenanceSchedule,
    MaintenanceSubmission,
    ReviewMaintenance,
    StartMaintenance,
)

from .security import application, context

router = APIRouter(prefix='/api/v1/namespaces/{namespace_id}/maintenance')


@router.post("/runs/{run_id}/capture-output")
async def capture_output(
    request: Request,
    namespace_id: str,
    
    run_id: str,
    command: CaptureMaintenanceOutput,
) -> MaintenanceRun:
    return await application(request).maintenance.capture_retained(
        await context(request, namespace_id), run_id, command
    )


@router.post("/runs/{run_id}/edit")
async def edit(
    request: Request,
    namespace_id: str,
    
    run_id: str,
    command: EditMaintenanceProposal,
) -> MaintenanceRun:
    return await application(request).maintenance.edit_proposal(
        await context(request, namespace_id), run_id, command
    )


@router.post("/runs")
async def submit(
    request: Request, namespace_id: str,  command: StartMaintenance
) -> MaintenanceSubmission:
    return await application(request).maintenance.submit(
        await context(request, namespace_id), command
    )


@router.get("/runs")
async def runs(
    request: Request, namespace_id: str,  limit: int = 50
) -> tuple[MaintenanceRun, ...]:
    return await application(request).maintenance.list_runs(
        await context(request, namespace_id,  capability="read"), limit=limit
    )


@router.get("/runs/{run_id}")
async def run(request: Request, namespace_id: str,  run_id: str) -> MaintenanceRun:
    return await application(request).maintenance.read(
        await context(request, namespace_id,  capability="read"), run_id
    )


@router.post("/runs/{run_id}/review")
async def review(
    request: Request, namespace_id: str,  run_id: str, command: ReviewMaintenance
) -> MaintenanceRun:
    return await application(request).maintenance.review(
        await context(request, namespace_id), run_id, command
    )


@router.post("/runs/{run_id}/retry")
async def retry(
    request: Request, namespace_id: str,  run_id: str, command: Operation
) -> MaintenanceSubmission:
    return await application(request).maintenance.retry(
        await context(request, namespace_id), run_id, command.operation_id
    )


@router.post("/schedules")
async def create_schedule(
    request: Request, namespace_id: str,  command: CreateMaintenanceSchedule
) -> MaintenanceSchedule:
    return await application(request).maintenance.create_schedule(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.get("/schedules")
async def schedules(
    request: Request, namespace_id: str
) -> tuple[MaintenanceSchedule, ...]:
    return await application(request).maintenance.list_schedules(
        await context(request, namespace_id,  capability="read")
    )


@router.post("/schedules/{schedule_id}/stop")
async def stop_schedule(
    request: Request, namespace_id: str,  schedule_id: str, command: Operation
) -> MaintenanceSchedule:
    return await application(request).maintenance.stop_schedule(
        await context(request, namespace_id,  capability="manage"),
        schedule_id,
        command.operation_id,
    )
