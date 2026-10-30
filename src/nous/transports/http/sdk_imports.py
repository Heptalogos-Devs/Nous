"""SDK import conveniences delegate original selection and attempts to their owner."""

from fastapi import APIRouter, Request
from nous_sdk.contracts import AdmitFiles, ImportDocument
from nous_sdk.contracts.imports import FormObservation, TextSplitOptions
from nous_sdk.contracts.text_import import TextImport
from pydantic import BaseModel, ConfigDict, Field

from .sdk_common import OperationIdentity
from .security import application, context

router = APIRouter()


class TextPreview(BaseModel):
    model_config = ConfigDict(extra="forbid")
    document: ImportDocument
    overrides: TextSplitOptions | None = None


class SyncEmbeddings(OperationIdentity):
    limit: int = Field(default=5000, ge=1, le=10000)


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/imports/preview")
async def preview_text(request: Request, namespace_id: str, subject_id: str, command: TextPreview):
    return await application(request).imports.preview_text_import(
        await context(request, namespace_id, subject_id, capability="read"),
        command.document,
        command.overrides,
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/memories/form")
async def form_observation(
    request: Request, namespace_id: str, subject_id: str, command: FormObservation
):
    return await application(request).imports.form_observation(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/embeddings/sync")
async def sync_embeddings(
    request: Request, namespace_id: str, subject_id: str, command: SyncEmbeddings
):
    return await application(request).imports.embedding_sync(
        await context(request, namespace_id, subject_id), command.limit, command.operation_id
    )


@router.get(
    "/namespaces/{namespace_id}/subjects/{subject_id}/imports/catalogs/{catalog_id}/attempts"
)
async def import_attempts(request: Request, namespace_id: str, subject_id: str, catalog_id: str):
    return await application(request).imports.read_attempts(
        await context(request, namespace_id, subject_id, capability="read"), catalog_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/imports/file-jobs")
async def submit_admit_files(
    request: Request, namespace_id: str, subject_id: str, command: AdmitFiles
):
    return await application(request).imports.submit_admit_files(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/imports/text")
async def submit_text_import(
    request: Request, namespace_id: str, subject_id: str, command: TextImport
):
    return await application(request).imports.submit_text_import(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/imports/text/{request_id}")
async def text_import_result(request: Request, namespace_id: str, subject_id: str, request_id: str):
    return await application(request).imports.read_text_import(
        await context(request, namespace_id, subject_id, capability="read"), request_id
    )
