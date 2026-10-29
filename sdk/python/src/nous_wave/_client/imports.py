"""Typed imports client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Literal, overload
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from ..contracts import (
    AdmitFiles,
    CatalogView,
    FreezeCatalog,
    ImportDocument,
    ImportResult,
    MediaReference,
    ProcessCatalog,
)
from ..contracts.imports import (
    EmbeddingSyncResult,
    FormObservation,
    ImportUnit,
    ObservationFormation,
    TextImportPreview,
    TextSplitOptions,
    UnitAttempt,
)
from ..contracts.text_import import TextImport, TextImportResult, TextImportSubmission
from ..operations import encoded, segment
from ..types import (
    FileImportSubmission,
    ImportSubmission,
    JsonObject,
)
from .import_inputs import document as input_document
from .import_inputs import normalize_inputs, split_options, validate_flags
from .import_inputs import origin as input_origin
from .jobs import JobClient
from .requests import request_scope


class ImportClient(JobClient):
    async def process_import_units(
        self,
        namespace_id: str | Mapping[str, JsonValue],
        subject_id: str | None = None,
        request: ProcessCatalog | Mapping[str, JsonValue] | None = None,
        *,
        operation_id: str | None = None,
    ) -> ImportSubmission:
        if isinstance(namespace_id, Mapping):
            if subject_id is not None or request is not None:
                raise ValueError("A Mapping import request carries its own scope")
            values = dict(namespace_id)
            if "llm_enabled" not in values:
                raise ValueError("Unit processing requires an explicit llm_enabled policy")
            validate_flags(values)
            namespace_id, subject_id = await request_scope(self, values)
            request = values
        if subject_id is None or request is None:
            raise ValueError("Unit processing requires scope and a frozen catalog command")
        if isinstance(request, ProcessCatalog):
            command = request
        else:
            values = dict(request)
            if "extract_time" in values:
                overrides = dict(
                    TypeAdapter(dict[str, JsonValue]).validate_python(
                        values.pop("formation_options_override", {})
                    )
                )
                extract_time = values.pop("extract_time")
                if "extract_time" in overrides and overrides["extract_time"] != extract_time:
                    raise ValueError("Conflicting formation time extraction options")
                overrides["extract_time"] = extract_time
                values["formation_options_override"] = overrides
            for key in ("origin", "retry_parent_job_id"):
                if key in values and values[key] is None:
                    values.pop(key)
            if "origin" in values:
                values["origin"] = {
                    key: item for key, item in input_origin(values["origin"]).items()
                }
            values["operation_id"] = operation_id or values.get("operation_id") or str(uuid4())
            command = ProcessCatalog.model_validate(values)
        return await self.submit_catalog(
            namespace_id,
            subject_id,
            command.model_copy(update={"operation_id": operation_id}) if operation_id else command,
        )

    async def freeze_import_catalog(
        self,
        namespace_id: str | Mapping[str, JsonValue],
        subject_id: str | None = None,
        inputs: Sequence[ImportDocument] | None = None,
        *,
        operation_id: str | None = None,
        chunk_size: int = 2400,
        chunk_overlap: int = 200,
    ) -> CatalogView:
        values: JsonObject
        if isinstance(namespace_id, Mapping):
            if subject_id is not None or inputs is not None:
                raise ValueError("A Mapping import request carries its own scope")
            values = dict(namespace_id)
            namespace_id, subject_id = await request_scope(self, values)
            split_options(values)
            normalize_inputs(values)
        else:
            if subject_id is None or inputs is None:
                raise ValueError("Catalog freezing requires scope and exact documents")
            values = {
                "inputs": [encoded(item) for item in inputs],
                "chunk_size": chunk_size,
                "chunk_overlap": chunk_overlap,
            }
        values["operation_id"] = operation_id or values.get("operation_id") or str(uuid4())
        return await self.freeze_catalog(
            namespace_id, subject_id, FreezeCatalog.model_validate(values)
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
    ) -> TextImportSubmission:
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
    ) -> TextImportSubmission:
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

    async def submit_admit_documents(
        self, namespace_id: str, subject_id: str, command: AdmitFiles
    ) -> FileImportSubmission:
        return FileImportSubmission.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/imports/file-jobs",
                body=encoded(command),
            )
        )

    @overload
    async def admit_import_files(
        self,
        namespace_id: Mapping[str, JsonValue],
        *,
        operation_id: str | None = None,
    ) -> FileImportSubmission: ...

    @overload
    async def admit_import_files(
        self,
        namespace_id: str,
        subject_id: str,
        request: Mapping[str, JsonValue],
        *,
        operation_id: str | None = None,
    ) -> ImportResult: ...

    async def admit_import_files(
        self,
        namespace_id: str | Mapping[str, JsonValue],
        subject_id: str | None = None,
        request: Mapping[str, JsonValue] | None = None,
        *,
        operation_id: str | None = None,
    ) -> ImportResult | FileImportSubmission:
        from ..contracts import AdmitFiles

        mapping_request = isinstance(namespace_id, Mapping)
        if isinstance(namespace_id, Mapping):
            if subject_id is not None or request is not None:
                raise ValueError("A Mapping import request carries its own scope")
            values = dict(namespace_id)
            if "llm_enabled" not in values:
                raise ValueError("File admission requires an explicit llm_enabled policy")
            namespace_id, subject_id = await request_scope(self, values)
        else:
            if subject_id is None or request is None:
                raise ValueError("File admission requires scope and documents")
            values = dict(request)
        validate_flags(values)
        split_options(values)
        normalize_inputs(values)
        values["operation_id"] = operation_id or values.get("operation_id") or str(uuid4())
        command = AdmitFiles.model_validate(values)
        if mapping_request:
            return await self.submit_admit_documents(namespace_id, subject_id, command)
        return await self.admit_documents(namespace_id, subject_id, command)

    async def submit_text_import(
        self, namespace_id: str, subject_id: str, command: TextImport
    ) -> TextImportSubmission:
        return TextImportSubmission.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/imports/text",
                body=encoded(command),
            )
        )

    async def text_import_result(
        self, namespace_id: str, subject_id: str, request_id: str
    ) -> TextImportResult:
        return TextImportResult.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/imports/text/{segment(request_id)}",
            )
        )

    async def import_text(
        self, request: Mapping[str, JsonValue], *, operation_id: str | None = None
    ) -> TextImportSubmission:
        values = dict(request)
        validate_flags(values)
        namespace, subject = await request_scope(self, values)
        normalize_inputs(values)
        values["operation_id"] = operation_id or values.get("operation_id") or str(uuid4())
        return await self.submit_text_import(namespace, subject, TextImport.model_validate(values))

    async def preview_text_import(
        self,
        namespace_id: str,
        subject_id: str,
        document: ImportDocument | Mapping[str, JsonValue],
        *,
        overrides: TextSplitOptions | Mapping[str, JsonValue] | None = None,
    ) -> TextImportPreview:
        selected = document if isinstance(document, ImportDocument) else input_document(document)
        if isinstance(overrides, Mapping) and "strategy" in overrides:
            fields: JsonObject = {"overrides": dict(overrides)}
            split_options(fields)
            options = TextSplitOptions.model_validate(fields)
        else:
            options = (
                overrides
                if isinstance(overrides, TextSplitOptions) or overrides is None
                else TextSplitOptions.model_validate(overrides)
            )
        return TextImportPreview.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/imports/preview",
                body={
                    "document": encoded(selected),
                    "overrides": encoded(options) if options is not None else None,
                },
            )
        )

    async def retry_import_inputs(
        self, namespace_id: str, subject_id: str, catalog_id: str, unit_ids: Sequence[str]
    ) -> tuple[ImportUnit, ...]:
        catalog = await self.import_catalog(namespace_id, subject_id, catalog_id)
        available = {unit.id: unit for unit in catalog.units}
        if len(set(unit_ids)) != len(unit_ids) or any(
            identity not in available for identity in unit_ids
        ):
            raise ValueError("Retry requires distinct actual UnitIds from the frozen catalog")
        return tuple(available[identity] for identity in unit_ids)

    async def import_attempts(
        self, namespace_id: str, subject_id: str, catalog_id: str
    ) -> tuple[UnitAttempt, ...]:
        return TypeAdapter(tuple[UnitAttempt, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/imports/catalogs/{segment(catalog_id)}/attempts",
            )
        )

    async def form_memory(
        self,
        subject_id: str,
        observation_id: str,
        *,
        source: MediaReference | Mapping[str, JsonValue] | None = None,
        operation_id: str | None = None,
        llm_enabled: bool = True,
    ) -> ObservationFormation:
        namespace = self.namespace_id or (await self.subject(subject_id)).namespace_id
        media = (
            source
            if isinstance(source, MediaReference) or source is None
            else MediaReference.model_validate(source)
        )
        command = FormObservation(
            operation_id=operation_id or str(uuid4()),
            observation_id=observation_id,
            source=media,
            llm_enabled=llm_enabled,
        )
        return ObservationFormation.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace, subject_id) + "/memories/form",
                body=encoded(command),
            )
        )

    async def embedding_sync(
        self, subject_id: str, *, limit: int = 5000, operation_id: str | None = None
    ) -> EmbeddingSyncResult:
        namespace = self.namespace_id or (await self.subject(subject_id)).namespace_id
        return EmbeddingSyncResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace, subject_id) + "/embeddings/sync",
                body={"operation_id": operation_id or str(uuid4()), "limit": limit},
            )
        )
