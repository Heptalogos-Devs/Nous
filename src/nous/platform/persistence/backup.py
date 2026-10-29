"""Portable installation archives from one SQLite snapshot and its actual bytes."""

from __future__ import annotations

import hashlib
import os
import sqlite3
import tempfile
import zipfile
from contextlib import closing
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import IO, TYPE_CHECKING, Literal

from pydantic import BaseModel, ConfigDict, Field

from nous.platform.concurrency import owned_thread

from .authority import Authority

if TYPE_CHECKING:
    from nous.platform.content import ContentStore


class BackupObject(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    content_hash: str = Field(pattern=r"^[a-f0-9]{64}$")
    byte_len: int = Field(ge=0)


class BackupManifest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    format: Literal["nous-wave-installation"] = "nous-wave-installation"
    version: Literal[1] = 1
    created_at: str
    commit_seq: int = Field(ge=0)
    database: BackupObject
    objects: tuple[BackupObject, ...]


@dataclass(frozen=True)
class BackupResult:
    path: Path
    commit_seq: int
    object_count: int


def _snapshot_refs(database: Path) -> tuple[int, tuple[BackupObject, ...]]:
    with closing(sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)) as connection:
        if connection.execute("PRAGMA integrity_check").fetchone() != ("ok",):
            raise ValueError("Backup database failed its integrity check")
        row = connection.execute(
            "SELECT commit_seq FROM nous_commit_clock WHERE singleton=1"
        ).fetchone()
        if row is None:
            raise ValueError("Backup database has no authoritative commit sequence")
        objects: dict[str, int] = {}
        for digest, length in connection.execute(
            "SELECT DISTINCT content_hash,byte_len FROM nous_content_references"
        ):
            if digest in objects and objects[digest] != length:
                raise ValueError("Backup contains conflicting content lengths")
            objects[digest] = length
        # Installation restore never enables the native/pickle serialization mode.
        tables = [
            record[0]
            for record in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")
        ]
        for name in tables:
            quoted = '"' + name.replace('"', '""') + '"'
            columns = {record[1] for record in connection.execute(f"PRAGMA table_info({quoted})")}
            if (
                "serialization" in columns
                and connection.execute(
                    f"SELECT 1 FROM {quoted} WHERE serialization IS NOT NULL AND serialization<>'portable_json' LIMIT 1"
                ).fetchone()
            ):
                raise ValueError(
                    "Installation backup contains a non-portable workflow serialization"
                )
        return int(row[0]), tuple(
            BackupObject(content_hash=digest, byte_len=length)
            for digest, length in sorted(objects.items())
        )


def _copy_checked(
    source: IO[bytes], destination: IO[bytes], expected: BackupObject | None = None
) -> BackupObject:
    digest = hashlib.sha256()
    length = 0
    while block := source.read(256 * 1024):
        length += len(block)
        if expected is not None and length > expected.byte_len:
            raise ValueError("Backup entry exceeds its declared length")
        digest.update(block)
        destination.write(block)
    result = BackupObject(content_hash=digest.hexdigest(), byte_len=length)
    if expected is not None and result != expected:
        raise ValueError("Backup entry failed its size or SHA-256 identity check")
    return result


def _sync_directory(path: Path) -> None:
    if os.name == "posix":
        descriptor = os.open(path, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


class BackupService:
    def __init__(self, authority: Authority, content: ContentStore):
        if content.authority is not authority:
            raise ValueError("Backup database and content must belong to the same authority")
        self.authority = authority
        self.content = content

    async def create(self, destination: str | Path) -> BackupResult:
        target = await owned_thread(lambda: self._prepare_destination(Path(destination)))
        async with self.content.preserve_all():
            with tempfile.TemporaryDirectory(prefix="nous-backup-", dir=target.parent) as temporary:
                database = await self.authority.backup(Path(temporary) / "authority.sqlite")
                return await owned_thread(
                    lambda: self._pack(database, target, Path(temporary) / "installation.zip")
                )

    @staticmethod
    def _prepare_destination(destination: Path) -> Path:
        target = destination.resolve()
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            raise FileExistsError(target)
        return target

    def _pack(self, database: Path, target: Path, archive_path: Path) -> BackupResult:
        seq, objects = _snapshot_refs(database)
        with zipfile.ZipFile(
            archive_path, "x", compression=zipfile.ZIP_STORED, allowZip64=True
        ) as archive:
            with (
                database.open("rb") as source,
                archive.open("authority.sqlite", "w", force_zip64=True) as output,
            ):
                database_ref = _copy_checked(source, output)
            for item in objects:
                entry = f"objects/{item.content_hash[:2]}/{item.content_hash[2:]}"
                with (
                    self.content.path_for(item.content_hash).open("rb") as source,
                    archive.open(entry, "w", force_zip64=True) as output,
                ):
                    _copy_checked(source, output, item)
            manifest = BackupManifest(
                created_at=datetime.now(UTC).isoformat(),
                commit_seq=seq,
                database=database_ref,
                objects=objects,
            )
            archive.writestr("manifest.json", manifest.model_dump_json())
        with archive_path.open("rb") as archive:
            os.fsync(archive.fileno())
        os.link(archive_path, target)
        _sync_directory(target.parent)
        return BackupResult(target, seq, len(objects))

    @staticmethod
    async def restore(
        archive: str | Path, destination: str | Path, *, max_total_bytes: int = 100 * 1024**3
    ) -> BackupResult:
        """Restore a new instance directory; existing instances are never overwritten."""
        try:
            return await owned_thread(
                lambda: BackupService._restore(Path(archive), Path(destination), max_total_bytes)
            )
        except (zipfile.BadZipFile, KeyError, sqlite3.DatabaseError) as error:
            raise ValueError("The installation backup archive is invalid") from error

    @staticmethod
    def _restore(archive_path: Path, destination: Path, max_total_bytes: int) -> BackupResult:
        target = destination.resolve()
        if target.exists():
            raise FileExistsError(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="nous-restore-", dir=target.parent) as temporary:
            staging = Path(temporary) / "instance"
            (staging / "data").mkdir(parents=True)
            objects_root = staging / "content" / "objects"
            with zipfile.ZipFile(archive_path) as archive:
                inventory = archive.infolist()
                names = [entry.filename for entry in inventory]
                if (
                    len(names) != len(set(names))
                    or sum(entry.file_size for entry in inventory) > max_total_bytes
                ):
                    raise ValueError(
                        "Backup has duplicate entries or exceeds the restore size limit"
                    )
                manifest_entry = archive.getinfo("manifest.json")
                if manifest_entry.file_size > 16 * 1024 * 1024:
                    raise ValueError("Backup manifest exceeds its size limit")
                manifest = BackupManifest.model_validate_json(archive.read(manifest_entry))
                expected = {"manifest.json", "authority.sqlite"}
                for item in manifest.objects:
                    expected.add(f"objects/{item.content_hash[:2]}/{item.content_hash[2:]}")
                if set(names) != expected or len(manifest.objects) != len(
                    {item.content_hash for item in manifest.objects}
                ):
                    raise ValueError("Backup inventory differs from its manifest")
                entries = [
                    ("authority.sqlite", staging / "data" / "authority.sqlite", manifest.database)
                ]
                entries += [
                    (
                        f"objects/{item.content_hash[:2]}/{item.content_hash[2:]}",
                        objects_root / item.content_hash[:2] / item.content_hash[2:],
                        item,
                    )
                    for item in manifest.objects
                ]
                for entry, path, identity in entries:
                    if archive.getinfo(entry).file_size != identity.byte_len:
                        raise ValueError("Backup entry length differs from its manifest")
                    path.parent.mkdir(parents=True, exist_ok=True)
                    with archive.open(entry) as source, path.open("xb") as output:
                        _copy_checked(source, output, identity)
                        output.flush()
                        os.fsync(output.fileno())
                    if entry != "authority.sqlite":
                        path.chmod(0o444)
            seq, actual = _snapshot_refs(staging / "data" / "authority.sqlite")
            if seq != manifest.commit_seq or actual != tuple(
                sorted(manifest.objects, key=lambda item: item.content_hash)
            ):
                raise ValueError("Backup manifest differs from its authoritative database snapshot")
            # Rename the complete directory only after every byte and reference validates.
            if target.exists():
                raise FileExistsError(target)
            for directory, _children, _files in os.walk(staging, topdown=False):
                _sync_directory(Path(directory))
            os.rename(staging, target)
            _sync_directory(target.parent)
            return BackupResult(target, seq, len(actual))
