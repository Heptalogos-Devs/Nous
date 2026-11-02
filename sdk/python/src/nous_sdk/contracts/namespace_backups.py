"""Namespace backup archives and bounded resumable transfers."""

from pathlib import PurePosixPath
from typing import Literal

from pydantic import AwareDatetime, Field, JsonValue, field_validator, model_validator

from .common import Contract, Operation
from .identity import Namespace
from .execution import ExecutionSnapshot

NAMESPACE_BACKUP_FORMAT = "nous-namespace-backup"
NAMESPACE_BACKUP_FORMAT_VERSION = 1


class NamespaceBackupFile(Contract):
    path: str = Field(min_length=1, max_length=4096)
    size_bytes: int = Field(ge=0)
    sha256: str = Field(pattern=r"^[0-9a-f]{64}$")

    @field_validator("path")
    @classmethod
    def relative_path(cls, value: str) -> str:
        path = PurePosixPath(value)
        if (
            "\\" in value
            or path.is_absolute()
            or path.as_posix() != value
            or any(part in {"", ".", ".."} for part in value.split("/"))
        ):
            raise ValueError("Backup paths must be normalized relative POSIX paths")
        return value


class NamespaceBackupManifest(Contract):
    format: Literal["nous-namespace-backup"] = NAMESPACE_BACKUP_FORMAT
    format_version: Literal[1] = NAMESPACE_BACKUP_FORMAT_VERSION
    backup_id: str = Field(pattern=r"^[0-9a-f]{32}$")
    source_namespace_id: str
    created_at: AwareDatetime
    producer_version: str
    source_config_version: int = Field(ge=0)
    namespace: Namespace
    data_size_bytes: int = Field(ge=0)
    files: tuple[NamespaceBackupFile, ...]

    @model_validator(mode="after")
    def file_index(self):
        if (
            len({item.path for item in self.files}) != len(self.files)
            or sum(item.size_bytes for item in self.files) != self.data_size_bytes
        ):
            raise ValueError("Backup file inventory is inconsistent")
        if self.namespace.id != self.source_namespace_id:
            raise ValueError("Backup Namespace identity is inconsistent")
        return self


class NamespaceBackupInfo(Contract):
    backup_id: str = Field(pattern=r"^[0-9a-f]{32}$")
    source_namespace_id: str
    created_at: AwareDatetime
    format_version: int
    producer_version: str
    source_config_version: int
    archive_size_bytes: int = Field(ge=0)
    data_size_bytes: int = Field(ge=0)
    file_count: int = Field(ge=0)
    sha256: str = Field(pattern=r"^[0-9a-f]{64}$")


class NamespaceBackupUpload(Contract):
    upload_id: str = Field(pattern=r"^[0-9a-f]{32}$")
    next_offset: int = Field(default=0, ge=0)


class NamespaceBackupChunk(Contract):
    backup: NamespaceBackupInfo
    offset: int
    data_base64: str
    next_offset: int
    complete: bool


class NamespaceBackupsPage(Contract):
    items: tuple[NamespaceBackupInfo, ...]
    next_cursor: str | None = None


class RestoreNamespaceBackup(Operation):
    backup_id: str
    preserve_ids: bool = False


class NamespaceBackupRestored(Contract):
    namespace: Namespace
    mapping: dict[str, str]
    resumed_jobs: tuple[str, ...] = ()
    authority_commit_seq: int
    frozen_invocations: int = 0


class NamespaceState(Contract):
    format: Literal["nous.namespace-state"] = "nous.namespace-state"
    version: Literal[1] = 1
    namespace_id: str
    authority_commit_seq: int
    tables: dict[str, tuple[dict[str, JsonValue], ...]]
    execution: ExecutionSnapshot
