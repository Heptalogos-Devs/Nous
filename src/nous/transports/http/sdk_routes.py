"""Public SDK resource routes backed by the shared domain owners."""


from fastapi import APIRouter, Request
from nous_sdk.contracts import (
    BlobDocument,
    DomainError,
    TimePoint,
    UnitOutput,
)
from nous_sdk.contracts.diagnostics import NamespaceDiagnostics
from nous_sdk.contracts.lifecycle import MemoryChange
from nous_sdk.contracts.references import (
    ExactReference,
)
from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy import select

from nous.imports.schema import outputs
from nous.material.service import require_observation_in

from .sdk_cognition import router as cognition_router
from .sdk_common import (
    OperationIdentity,
)
from .sdk_graph import router as graph_router
from .sdk_imports import router as import_router
from .sdk_media import router as media_router
from .sdk_social import router as social_router
from .sdk_sources import router as sources_router
from .sdk_vectors import router as vectors_router
from .security import application, authorize, context

router = APIRouter(prefix="/api/knowledge")
router.include_router(import_router)
router.include_router(media_router)
router.include_router(social_router)
router.include_router(sources_router)
router.include_router(graph_router)
router.include_router(cognition_router)
router.include_router(vectors_router)


class Observe(BaseModel):
    model_config = ConfigDict(extra="forbid")

    material_id: str
    observed_at: TimePoint | None = None
    origin: dict[str, str] = Field(default_factory=dict)


class ReadReference(BaseModel):
    model_config = ConfigDict(extra="forbid")

    reference: ExactReference | str
    as_of_commit: int | None = None


@router.get("/query/manual")
async def manual(request: Request):
    from nous_native import nousql_manual

    await authorize(request, capability="read")
    return nousql_manual()


@router.post("/namespaces/{namespace_id}/references")
async def resolve_reference(request: Request, namespace_id: str, command: ReadReference):
    ctx = await context(request, namespace_id, capability="read")
    locator = command.reference if isinstance(command.reference, str) else command.reference.exact()
    return await application(request).references.resolve(ctx, locator, command.as_of_commit)


@router.post("/namespaces/{namespace_id}/observations")
async def observe(request: Request, namespace_id: str, command: Observe):
    app = application(request)
    ctx = await context(request, namespace_id, capability="write")
    async with app.authority.write(ctx) as tx:
        material = await app.material.read_in(tx, ctx, command.material_id)
        document = BlobDocument(
            filename=material["filename"],
            media_type=material["media_type"],
            source_type=material["source_type"],
            source=material.get("source"),
            origin=command.origin,
            observed_at=command.observed_at,
        )
        admission = await app.material.admit_observation_in(tx, ctx, command.material_id, document)
        return admission.event


@router.get("/namespaces/{namespace_id}/imports/outputs/{output_id}")
async def import_output(request: Request, namespace_id: str, output_id: str):
    app = application(request)
    ctx = await context(request, namespace_id, capability="read")
    async with app.authority.read(ctx) as view:
        raw = (
            await view.connection.execute(select(outputs.c.record).where(outputs.c.id == output_id))
        ).scalar_one_or_none()
        if raw is None:
            raise DomainError("output_not_found", "Import output is unavailable")
        output = UnitOutput.model_validate(raw)
        await require_observation_in(view, ctx, output.observation_id)
        for ref in output.cognition:
            await app.cognition.get_in(view, ctx, ref)
        return output


@router.post("/namespaces/{namespace_id}/memories/change")
async def memory_change(request: Request, namespace_id: str, command: MemoryChange):
    return await application(request).memories.change_memory(
        await context(request, namespace_id), command
    )


@router.get("/namespaces/{namespace_id}/diagnostics")
async def subject_diagnostics(request: Request, namespace_id: str) -> NamespaceDiagnostics:
    ctx = await context(request, namespace_id, capability="read")
    return await application(request).diagnostics.read(ctx)


@router.post("/namespaces/{namespace_id}/references/reconcile")
async def reconcile_references(request: Request, namespace_id: str, command: OperationIdentity):
    return await application(request).references.reconcile(
        await context(request, namespace_id), command.operation_id
    )
