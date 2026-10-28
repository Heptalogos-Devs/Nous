"""Typed imports client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Literal
from uuid import uuid4

from pydantic import JsonValue

from ..contracts import (
    CatalogView,
    FreezeCatalog,
    ImportDocument,
    ImportResult,
    ProcessCatalog,
)
from ..types import (
    ImportSubmission,
)
from .jobs import JobClient


class ImportClient(JobClient):
    async def process_import_units(
        self,
        namespace_id: str,
        subject_id: str,
        request: ProcessCatalog | Mapping[str, JsonValue],
        *,
        operation_id: str | None = None,
    ) -> ImportSubmission:
        command = (
            request
            if isinstance(request, ProcessCatalog)
            else ProcessCatalog.model_validate(request)
        )
        return await self.submit_catalog(
            namespace_id,
            subject_id,
            command.model_copy(update={"operation_id": operation_id})
            if operation_id is not None
            else command,
        )

    async def freeze_import_catalog(
        self,
        namespace_id: str,
        subject_id: str,
        inputs: Sequence[ImportDocument],
        *,
        operation_id: str | None = None,
        chunk_size: int = 2400,
        chunk_overlap: int = 200,
    ) -> CatalogView:
        return await self.freeze_catalog(
            namespace_id,
            subject_id,
            FreezeCatalog(
                inputs=tuple(inputs),
                operation_id=operation_id or str(uuid4()),
                chunk_size=chunk_size,
                chunk_overlap=chunk_overlap,
            ),
        )

    async def import_files(
        self,
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
    ) -> ImportResult:
        from ..imports import import_files

        return await import_files(
            self,
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

    async def import_directory(
        self,
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
    ) -> ImportResult:
        from ..imports import import_directory

        return await import_directory(
            self,
            namespace_id,
            subject_id,
            transfer_root=transfer_root,
            alias=alias,
            relative_path=relative_path,
            pattern=pattern,
            recursive=recursive,
            input_format=input_format,
            form_memory=form_memory,
            completion=completion,
            origin=origin,
            operation_id=operation_id,
        )

    async def admit_import_files(
        self,
        namespace_id: str,
        subject_id: str,
        request: Mapping[str, JsonValue],
        *,
        operation_id: str | None = None,
    ) -> ImportResult:
        from ..contracts import AdmitFiles

        return await self.admit_documents(
            namespace_id,
            subject_id,
            AdmitFiles.model_validate(
                {
                    **request,
                    "operation_id": operation_id or request.get("operation_id") or str(uuid4()),
                }
            ),
        )

    async def import_text(
        self, request: Mapping[str, JsonValue], *, operation_id: str | None = None
    ) -> ImportResult:
        from ..contracts import AdmitFiles

        namespace_id = request.get("namespace_id") or self.namespace_id
        subject_id = request.get("subject_id") or self.subject_id
        if not isinstance(namespace_id, str) or not isinstance(subject_id, str):
            raise ValueError("Text import requires namespace_id and subject_id")
        command = AdmitFiles.model_validate(
            {
                "inputs": request.get("inputs"),
                "llm_enabled": request.get("form_memory", False),
                "vectors": request.get(
                    "vectors", "metadata_allowed" if request.get("form_memory") else "disabled"
                ),
                "operation_id": operation_id or request.get("operation_id") or str(uuid4()),
            }
        )
        return await self.admit_documents(namespace_id, subject_id, command)
