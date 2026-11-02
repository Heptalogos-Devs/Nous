"""Typed native Namespace backup and transfer operations."""

import base64

from ..contracts.namespace_backups import (
    NamespaceBackupChunk,
    NamespaceBackupInfo,
    NamespaceBackupRestored,
    NamespaceBackupsPage,
    NamespaceBackupUpload,
    RestoreNamespaceBackup,
)
from ..operations import encoded, segment
from .core import ClientCore


class NamespaceBackupClient(ClientCore):
    async def create_namespace_backup(self, namespace_id: str) -> NamespaceBackupInfo:
        return NamespaceBackupInfo.model_validate(
            await self.request("POST", f"/api/v1/namespaces/{segment(namespace_id)}/backups")
        )

    async def namespace_backup(self, backup_id: str) -> NamespaceBackupInfo:
        return NamespaceBackupInfo.model_validate(
            await self.request("GET", f"/api/v1/namespace-backups/{segment(backup_id)}")
        )

    async def namespace_backups(
        self, namespace_id: str, *, limit: int = 100, cursor: str | None = None
    ) -> NamespaceBackupsPage:
        return NamespaceBackupsPage.model_validate(
            await self.request(
                "GET",
                f"/api/v1/namespaces/{segment(namespace_id)}/backups",
                params={"limit": limit, "cursor": cursor},
            )
        )

    async def delete_namespace_backup(self, backup_id: str) -> None:
        await self.request("DELETE", f"/api/v1/namespace-backups/{segment(backup_id)}")

    async def download_namespace_backup(
        self, backup_id: str, *, offset: int = 0, max_bytes: int = 262144
    ) -> NamespaceBackupChunk:
        return NamespaceBackupChunk.model_validate(
            await self.request(
                "GET",
                f"/api/v1/namespace-backups/{segment(backup_id)}/content",
                params={"offset": offset, "max_bytes": max_bytes},
            )
        )

    async def begin_namespace_backup_upload(self) -> NamespaceBackupUpload:
        return NamespaceBackupUpload.model_validate(
            await self.request("POST", "/api/v1/namespace-backup-uploads")
        )

    async def upload_namespace_backup_chunk(
        self, upload_id: str, offset: int, data: bytes
    ) -> NamespaceBackupUpload:
        return NamespaceBackupUpload.model_validate(
            await self.request(
                "PUT",
                f"/api/v1/namespace-backup-uploads/{segment(upload_id)}",
                body={"offset": offset, "data_base64": base64.b64encode(data).decode("ascii")},
            )
        )

    async def complete_namespace_backup_upload(
        self, upload_id: str, expected_sha256: str = ""
    ) -> NamespaceBackupInfo:
        return NamespaceBackupInfo.model_validate(
            await self.request(
                "POST",
                f"/api/v1/namespace-backup-uploads/{segment(upload_id)}/complete",
                body={"expected_sha256": expected_sha256},
            )
        )

    async def abort_namespace_backup_upload(self, upload_id: str) -> None:
        await self.request("DELETE", f"/api/v1/namespace-backup-uploads/{segment(upload_id)}")

    async def restore_namespace_backup(
        self, namespace_id: str, command: RestoreNamespaceBackup
    ) -> NamespaceBackupRestored:
        return NamespaceBackupRestored.model_validate(
            await self.request(
                "POST",
                f"/api/v1/namespaces/{segment(namespace_id)}/restore-backup",
                body=encoded(command),
            )
        )
