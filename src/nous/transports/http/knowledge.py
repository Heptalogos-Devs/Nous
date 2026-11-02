"""Source lifecycle, bundles and working contexts use the same authority."""

from typing import Literal

from fastapi import APIRouter, Request
from nous_sdk.contracts import (
    BundleRequest,
    ContextCommand,
    ContextFeedback,
    InstallBundle,
    SourceCommand,
)
from nous_sdk.contracts.management import SourceSelection

from .security import application, context

router = APIRouter(prefix='/api/v1/namespaces/{namespace_id}')


@router.get("/sources")
async def sources(request: Request, namespace_id: str):
    return await application(request).sources.list(
        await context(request, namespace_id,  capability="read")
    )


@router.post("/sources/preview")
async def preview(
    request: Request,
    namespace_id: str,
    
    command: SourceSelection,
    action: Literal["trash", "restore", "purge"] = "trash",
):
    return await application(request).sources.preview(
        await context(request, namespace_id,  capability="read"),
        command.observation_ids,
        references=command.references,
        source_names=command.source_names,
        records=command.records,
        retention_expectations=command.retention_expectations,
        action=action,
    )


@router.post("/sources/trash")
async def trash(request: Request, namespace_id: str,  command: SourceCommand):
    return await application(request).sources.trash(
        await context(request, namespace_id), command
    )


@router.post("/sources/restore")
async def restore(request: Request, namespace_id: str,  command: SourceCommand):
    return await application(request).sources.restore(
        await context(request, namespace_id), command
    )


@router.post("/sources/purge")
async def purge(request: Request, namespace_id: str,  command: SourceCommand):
    return await application(request).sources.purge(
        await context(request, namespace_id), command
    )


@router.post("/bundles/export")
async def export_bundle(
    request: Request, namespace_id: str,  command: BundleRequest
):
    return await application(request).bundles.export(
        await context(request, namespace_id,  capability="read"), command
    )


@router.post("/bundles/install")
async def install_bundle(
    request: Request, namespace_id: str,  command: InstallBundle
):
    return await application(request).bundles.install(
        await context(request, namespace_id), command
    )


@router.post("/contexts")
async def admit_context(
    request: Request, namespace_id: str,  command: ContextCommand
):
    return await application(request).contexts.admit(
        await context(request, namespace_id), command
    )


@router.get("/contexts/{context_id}")
async def read_context(request: Request, namespace_id: str,  context_id: str):
    return await application(request).contexts.read(
        await context(request, namespace_id,  capability="read"), context_id
    )


@router.post("/contexts/feedback")
async def context_feedback(
    request: Request, namespace_id: str,  command: ContextFeedback
):
    return await application(request).contexts.record_feedback(
        await context(request, namespace_id), command
    )
