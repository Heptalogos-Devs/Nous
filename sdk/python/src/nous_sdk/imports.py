"""Upload exact originals and pass native format/identity to the import owner."""

import mimetypes
import sqlite3
import tempfile
from collections.abc import Mapping, Sequence
from contextlib import asynccontextmanager, closing
from pathlib import Path
from typing import TYPE_CHECKING, Literal
from uuid import NAMESPACE_URL, uuid4, uuid5

import anyio
from pydantic import JsonValue

from .contracts import ImportDocument
from .contracts.common import FormationPurpose
from .contracts.imports import ImportFormat
from .contracts.media import MediaStrategy
from .contracts.text_import import TextImport, TextImportSubmission
from .files import ManagedFiles, file_name
from .operations import encoded
from .types import LocalImportFile

if TYPE_CHECKING:
    from ._client.imports import ImportClient
    from ._client.materials import MaterialClient


def _backup_sqlite(source: Path, destination: Path) -> None:
    def progress(_status: int, _remaining: int, _total: int) -> None:
        anyio.from_thread.check_cancelled()

    with (
        closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as original,
        closing(sqlite3.connect(destination)) as snapshot,
    ):
        original.backup(snapshot, pages=256, progress=progress)


@asynccontextmanager
async def _captured_file(path: Path, format_: str):
    if format_ != "sqlite":
        yield path
        return
    with tempfile.TemporaryDirectory(prefix="nous-sqlite-source-") as directory:
        snapshot = Path(directory) / "source.sqlite"
        await anyio.to_thread.run_sync(_backup_sqlite, path, snapshot)
        yield snapshot


def _local_input(
    managed: ManagedFiles, value: str | Path | Mapping[str, JsonValue]
) -> tuple[LocalImportFile, Path]:
    item = (
        LocalImportFile.model_validate(value)
        if isinstance(value, Mapping)
        else LocalImportFile(path=Path(value))
    )
    path = item.staged_path or item.path
    if path is None:
        raise ValueError("Import file requires staged_path or path")
    return item, managed.local_file(path)


async def upload_import_inputs(
    client: "MaterialClient",
    namespace_id: str,
    
    files: Sequence[str | Path | Mapping[str, JsonValue]],
    *,
    transfer_root: str | Path,
    input_format: ImportFormat | None = None,
    operation_id: str | None = None,
) -> tuple[ImportDocument, ...]:
    managed = ManagedFiles(transfer_root)
    pending: list[tuple[LocalImportFile, Path, str, str]] = []
    for value in files:
        item, path = _local_input(managed, value)
        name = file_name(item.filename or item.name or path.name)
        format_ = (
            item.format
            or input_format
            or {
                ".txt": "text",
                ".md": "markdown",
                ".markdown": "markdown",
                ".html": "html",
                ".htm": "html",
                ".pdf": "pdf",
                ".json": "json",
                ".jsonl": "jsonl",
                ".parquet": "parquet",
                ".sqlite": "sqlite",
                ".sqlite3": "sqlite",
                ".db": "sqlite",
                ".graphml": "graphml",
            }.get(Path(name).suffix.lower())
        )
        if format_ is None and (mimetypes.guess_type(name)[0] or "").startswith(
            ("image/", "audio/", "video/")
        ):
            format_ = "media"
        if format_ not in {
            "text",
            "markdown",
            "html",
            "pdf",
            "json",
            "jsonl",
            "media",
            "parquet",
            "sqlite",
            "graphml",
        }:
            raise ValueError("Select a supported document or media format")
        pending.append((item, path, name, format_))
    if not pending:
        raise ValueError("Select at least one import file")
    documents: list[ImportDocument] = []
    for index, (item, path, name, format_) in enumerate(pending):
        async with _captured_file(path, format_) as captured:
            receipt = await client.upload_file(
                namespace_id,
                captured,
                
                media_type={
                    "text": "text/plain",
                    "markdown": "text/markdown",
                    "html": "text/html",
                    "pdf": "application/pdf",
                    "media": mimetypes.guess_type(name)[0] or "application/octet-stream",
                    "json": "application/json",
                    "jsonl": "application/x-ndjson",
                    "parquet": "application/vnd.apache.parquet",
                    "sqlite": "application/vnd.sqlite3",
                    "graphml": "application/graphml+xml",
                }[format_],
                source=name,
                operation_id=str(
                    uuid5(
                        NAMESPACE_URL,
                        f"nous-wave:import-upload:{namespace_id}:{namespace_id}:{operation_id}:{index}",
                    )
                )
                if operation_id is not None
                else None,
            )
        documents.append(
            ImportDocument.model_validate(
                {
                    "material_id": receipt.result.id,
                    "filename": name,
                    "format": format_,
                    "source_type": "document",
                    "source": item.source if "source" in item.model_fields_set else name,
                    "file_identity": item.file_identity,
                }
            )
        )
    if not documents:
        raise ValueError("Select at least one import file")
    return tuple(documents)


async def upload_import_file_documents(
    client: "MaterialClient",
    namespace_id: str,
    
    files: Sequence[str | Path | Mapping[str, JsonValue]],
    *,
    transfer_root: str | Path,
    input_format: ImportFormat | None = None,
    operation_id: str | None = None,
    alias: str | None = None,
) -> tuple[ImportDocument, ...]:
    managed = ManagedFiles(transfer_root)
    identities: list[str | None] = []
    for value in files:
        item, path = _local_input(managed, value)
        identities.append(
            item.file_identity
            if item.file_identity is not None
            else (managed.file_identity(alias, path) if alias is not None else None)
        )
    documents = await upload_import_inputs(
        client,
        namespace_id,
        
        files,
        transfer_root=transfer_root,
        input_format=input_format,
        operation_id=operation_id,
    )
    return tuple(
        document.model_copy(update={"file_identity": identity})
        for document, identity in zip(documents, identities, strict=True)
    )


async def import_files(
    client: "ImportClient",
    namespace_id: str,
    
    files: Sequence[str | Path | Mapping[str, JsonValue]],
    *,
    transfer_root: str | Path,
    input_format: ImportFormat | None = None,
    formation_purpose: FormationPurpose | None = None,
    media_strategy: MediaStrategy = "direct_structured",
    completion: Literal["accepted", "formed"] = "accepted",
    origin: Mapping[str, str] | None = None,
    operation_id: str | None = None,
) -> TextImportSubmission:
    if completion == "formed" and formation_purpose is None:
        raise ValueError("formed import completion requires a formation purpose")
    operation = operation_id or str(uuid4())
    documents = await upload_import_inputs(
        client,
        namespace_id,
        
        files,
        transfer_root=transfer_root,
        input_format=input_format,
        operation_id=operation,
    )
    inputs = tuple(
        document.model_copy(
            update={"origin": dict(origin or {}), "media_strategy": media_strategy}
        )
        for document in documents
    )
    command = TextImport.model_validate(
        {
            "operation_id": operation,
            "inputs": [encoded(document) for document in inputs],
            "formation_purpose": formation_purpose,
            "completion": completion,
            "vectors": "metadata_allowed" if formation_purpose is not None else "disabled",
        }
    )
    return await client.submit_text_import(namespace_id,  command)


async def import_directory(
    client: "ImportClient",
    namespace_id: str,
    
    *,
    transfer_root: str | Path,
    alias: str = "raw",
    relative_path: str = "",
    pattern: str = "*",
    recursive: bool = True,
    input_format: ImportFormat | None = None,
    formation_purpose: FormationPurpose | None = None,
    media_strategy: MediaStrategy = "direct_structured",
    completion: Literal["accepted", "formed"] = "accepted",
    origin: Mapping[str, str] | None = None,
    operation_id: str | None = None,
) -> TextImportSubmission:
    files = await ManagedFiles(transfer_root).scan(
        alias, relative_path, pattern=pattern, recursive=recursive
    )
    return await import_files(
        client,
        namespace_id,
        
        files,
        transfer_root=transfer_root,
        input_format=input_format,
        formation_purpose=formation_purpose,
        media_strategy=media_strategy,
        completion=completion,
        origin=origin,
        operation_id=operation_id,
    )
