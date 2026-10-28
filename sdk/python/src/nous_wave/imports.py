"""Upload exact originals and pass native format/identity to the import owner."""

from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Literal
from uuid import NAMESPACE_URL, uuid4, uuid5

from pydantic import JsonValue

from .contracts import AdmitFiles, ImportDocument, ImportResult
from .files import ManagedFiles, file_name
from .operations import encoded
from .types import LocalImportFile, SkippedFiles

if TYPE_CHECKING:
    from .client import NousClient


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
    client: "NousClient",
    namespace_id: str,
    subject_id: str,
    files: Sequence[str | Path | Mapping[str, JsonValue]],
    *,
    transfer_root: str | Path,
    input_format: Literal["text", "json", "jsonl"] | None = None,
    operation_id: str | None = None,
) -> tuple[ImportDocument, ...]:
    managed = ManagedFiles(transfer_root)
    documents: list[ImportDocument] = []
    for index, value in enumerate(files):
        item, path = _local_input(managed, value)
        name = file_name(item.filename or item.name or path.name)
        format_ = (
            item.format
            or input_format
            or {".txt": "text", ".md": "text", ".json": "json", ".jsonl": "jsonl"}.get(
                Path(name).suffix.lower()
            )
        )
        if format_ not in {"text", "json", "jsonl"}:
            raise ValueError("Import file format must be text, json or jsonl")
        receipt = await client.upload_file(
            namespace_id,
            path,
            owner_subject_id=subject_id,
            media_type={
                "text": "text/plain",
                "json": "application/json",
                "jsonl": "application/x-ndjson",
            }[format_],
            source=name,
            operation_id=str(
                uuid5(
                    NAMESPACE_URL,
                    f"nous-wave:import-upload:{namespace_id}:{subject_id}:{operation_id}:{index}",
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
                    "source_type": item.source or "document",
                    "file_identity": item.file_identity,
                }
            )
        )
    if not documents:
        raise ValueError("Select at least one import file")
    return tuple(documents)


async def upload_import_file_documents(
    client: "NousClient",
    namespace_id: str,
    subject_id: str,
    files: Sequence[str | Path | Mapping[str, JsonValue]],
    *,
    transfer_root: str | Path,
    input_format: Literal["text", "json", "jsonl"] | None = None,
    operation_id: str | None = None,
    alias: str | None = None,
) -> tuple[ImportDocument, ...]:
    managed = ManagedFiles(transfer_root)
    identities: list[str | None] = []
    for value in files:
        item, path = _local_input(managed, value)
        identities.append(
            item.file_identity
            or (managed.file_identity(alias, path) if alias is not None else None)
        )
    documents = await upload_import_inputs(
        client,
        namespace_id,
        subject_id,
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
    client: "NousClient",
    namespace_id: str,
    subject_id: str,
    files: Sequence[str | Path | Mapping[str, JsonValue]],
    *,
    transfer_root: str | Path,
    input_format: Literal["text", "json", "jsonl"] | None = None,
    form_memory: bool = False,
    completion: Literal["accepted", "formed"] = "accepted",
    origin: Mapping[str, str] | None = None,
    operation_id: str | None = None,
) -> ImportResult | SkippedFiles:
    if completion == "formed" and not form_memory:
        raise ValueError("formed import completion requires form_memory=True")
    operation = operation_id or str(uuid4())
    documents = await upload_import_inputs(
        client,
        namespace_id,
        subject_id,
        files,
        transfer_root=transfer_root,
        input_format=input_format,
        operation_id=operation,
    )
    inputs = tuple(
        document.model_copy(update={"origin": dict(origin or {})}) for document in documents
    )
    command = AdmitFiles.model_validate(
        {
            "operation_id": operation,
            "inputs": [encoded(document) for document in inputs],
            "llm_enabled": form_memory,
            "vectors": "metadata_allowed" if form_memory else "disabled",
        }
    )
    return await client.admit_documents(namespace_id, subject_id, command)


async def import_directory(
    client: "NousClient",
    namespace_id: str,
    subject_id: str,
    *,
    transfer_root: str | Path,
    alias: str = "raw",
    relative_path: str = "",
    pattern: str = "*",
    recursive: bool = True,
    input_format: Literal["text", "json", "jsonl"] | None = None,
    form_memory: bool = False,
    completion: Literal["accepted", "formed"] = "accepted",
    origin: Mapping[str, str] | None = None,
    operation_id: str | None = None,
) -> ImportResult | SkippedFiles:
    files = await ManagedFiles(transfer_root).scan(
        alias, relative_path, pattern=pattern, recursive=recursive
    )
    return await import_files(
        client,
        namespace_id,
        subject_id,
        files,
        transfer_root=transfer_root,
        input_format=input_format,
        form_memory=form_memory,
        completion=completion,
        origin=origin,
        operation_id=operation_id,
    )
