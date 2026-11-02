"""Every fact entry point delegates identity and transitions to one domain owner."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.facts import FactChange, FactQuery, FactRead

from .security import application, context

router = APIRouter(prefix='/api/knowledge/namespaces/{namespace_id}/facts')


@router.post("/change")
async def change(request: Request, namespace_id: str,  command: FactChange):
    return await application(request).facts.change(
        await context(request, namespace_id), command
    )


@router.post("/query")
async def query(request: Request, namespace_id: str,  command: FactQuery):
    return await application(request).facts.query(
        await context(request, namespace_id,  capability="read"), command
    )


@router.post("/get")
async def get(request: Request, namespace_id: str,  command: FactRead):
    return await application(request).facts.get(
        await context(request, namespace_id,  capability="read"), command.reference
    )
