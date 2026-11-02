"""Complete Namespace backup operations delegate to the native snapshot owner."""

import base64

from fastapi import APIRouter, Request
from nous.platform.call_context import CallContext
from nous_sdk.contracts.namespace_backups import (
    NamespaceBackupChunk,
    NamespaceBackupInfo,
    NamespaceBackupRestored,
    NamespaceBackupsPage,
    NamespaceBackupUpload,
    RestoreNamespaceBackup,
)
from pydantic import BaseModel, Field

from .identities import manager_context
from .security import application, authorize, context

router = APIRouter()


class UploadChunk(BaseModel):
    offset: int = Field(ge=0)
    data_base64: str = Field(max_length=1398104)


class CompleteUpload(BaseModel):
    expected_sha256: str = ""


async def backup_context(request: Request, backup_id: str) -> CallContext:
    await authorize(request, capability="manage")
    ns = await application(request).namespace_backups.namespace_for_backup(backup_id)
    return await manager_context(request, ns)


async def upload_context(request: Request, upload_id: str | None = None) -> CallContext:
    (await authorize(request, capability="manage")).require_instance_management()
    namespace = (
        await application(request).namespace_backups.namespace_for_upload(upload_id)
        if upload_id
        else "instance"
    )
    return await context(request, namespace, capability="manage", identity_purpose="management")


@router.post("/namespaces/{namespace_id}/backups")
async def create_backup(request: Request, namespace_id: str) -> NamespaceBackupInfo:
    return await application(request).namespace_backups.create(
        await manager_context(request, namespace_id)
    )


@router.get("/namespaces/{namespace_id}/backups")
async def list_backups(
    request: Request, namespace_id: str, limit: int = 100, cursor: str | None = None
) -> NamespaceBackupsPage:
    return await application(request).namespace_backups.list(
        await manager_context(request, namespace_id), limit, cursor
    )


@router.get("/namespace-backups/{backup_id}")
async def get_backup(request: Request, backup_id: str) -> NamespaceBackupInfo:
    return await application(request).namespace_backups.get(
        await backup_context(request, backup_id), backup_id
    )


@router.delete("/namespace-backups/{backup_id}")
async def delete_backup(request: Request, backup_id: str) -> dict[str, bool]:
    await application(request).namespace_backups.delete(
        await backup_context(request, backup_id), backup_id
    )
    return {"deleted": True}


@router.get("/namespace-backups/{backup_id}/content")
async def download_backup(
    request: Request, backup_id: str, offset: int = 0, max_bytes: int = 262144
) -> NamespaceBackupChunk:
    return await application(request).namespace_backups.download(
        await backup_context(request, backup_id), backup_id, offset, max_bytes
    )


@router.post("/namespace-backup-uploads")
async def begin_upload(request: Request) -> NamespaceBackupUpload:
    return await application(request).namespace_backups.begin_upload(await upload_context(request))


@router.put("/namespace-backup-uploads/{upload_id}")
async def upload_chunk(
    request: Request, upload_id: str, command: UploadChunk
) -> NamespaceBackupUpload:
    return await application(request).namespace_backups.upload_chunk(
        await upload_context(request, upload_id),
        upload_id,
        command.offset,
        base64.b64decode(command.data_base64, validate=True),
    )


@router.post("/namespace-backup-uploads/{upload_id}/complete")
async def complete_upload(
    request: Request, upload_id: str, command: CompleteUpload
) -> NamespaceBackupInfo:
    return await application(request).namespace_backups.complete_upload(
        await upload_context(request, upload_id), upload_id, command.expected_sha256
    )


@router.delete("/namespace-backup-uploads/{upload_id}")
async def abort_upload(request: Request, upload_id: str) -> dict[str, bool]:
    await application(request).namespace_backups.abort_upload(
        await upload_context(request, upload_id), upload_id
    )
    return {"aborted": True}


@router.post("/namespaces/{namespace_id}/restore-backup")
async def restore_backup(
    request: Request, namespace_id: str, command: RestoreNamespaceBackup
) -> NamespaceBackupRestored:
    (await authorize(request, capability="manage")).require_instance_management()
    return await application(request).namespace_backups.restore(
        await manager_context(request, namespace_id), command
    )
