"""Correction previews and adoption use the same saved domain plan."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.corrections import (
    ApplyCorrection,
    CorrectionPlansQuery,
    PlanCorrection,
    RollbackCorrection,
)

from .security import application, context

router = APIRouter(prefix='/api/knowledge/namespaces/{namespace_id}/corrections')


@router.post("/plan")
async def plan(request: Request, namespace_id: str,  command: PlanCorrection):
    return await application(request).corrections.plan(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.post("/apply")
async def apply(request: Request, namespace_id: str,  command: ApplyCorrection):
    return await application(request).corrections.apply(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.post("/rollback")
async def rollback(
    request: Request, namespace_id: str,  command: RollbackCorrection
):
    return await application(request).corrections.rollback(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.get("")
async def list_plans(
    request: Request, namespace_id: str,  cursor: str | None = None, limit: int = 50
):
    return await application(request).corrections.list(
        await context(request, namespace_id,  capability="manage"),
        CorrectionPlansQuery(cursor=cursor, limit=limit),
    )


@router.get("/{plan_id}")
async def get(request: Request, namespace_id: str,  plan_id: str):
    return await application(request).corrections.get(
        await context(request, namespace_id,  capability="manage"), plan_id
    )
