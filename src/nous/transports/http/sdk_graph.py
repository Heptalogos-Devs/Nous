"""Canonical graph management shares source-backed cognitive transactions."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.graph_management import GraphChange

from .security import application, context

router = APIRouter()


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/graph")
async def snapshot(request: Request, namespace_id: str, subject_id: str):
    return await application(request).cognition.graph_snapshot(
        await context(request, namespace_id, subject_id, capability="read")
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/graph")
async def change(request: Request, namespace_id: str, subject_id: str, command: GraphChange):
    return await application(request).cognition.manage_graph(
        await context(request, namespace_id, subject_id), command
    )
