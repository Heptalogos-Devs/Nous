"""Source lifecycle, bundles and working contexts use the same authority."""

from typing import Literal

from fastapi import APIRouter, Request
from nous_sdk.contracts import (
    BundleRequest,
    InstallBundle,
    SourceCommand,
)
from nous_sdk.contracts.management import SourceSelection

from .security import application, context

router = APIRouter(prefix='/api/knowledge/namespaces/{namespace_id}')


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


@router.get("/bundles/installations")
async def bundle_installations(request: Request, namespace_id: str):
    return await application(request).bundles.list_installs(
        await context(request, namespace_id, capability="read")
    )


@router.get("/bundles/installations/{installation_id}")
async def bundle_installation(request: Request, namespace_id: str, installation_id: str):
    return await application(request).bundles.get_installation(
        await context(request, namespace_id, capability="read"), installation_id
    )


@router.post("/bundles/installations/{installation_id}/uninstall")
async def uninstall_bundle(request: Request, namespace_id: str, installation_id: str):
    return await application(request).bundles.uninstall(
        await context(request, namespace_id, capability="manage"), installation_id
    )
