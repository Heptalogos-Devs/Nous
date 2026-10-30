"""Thin typed HTTP entrypoints into the shared domain owners."""

from uuid import uuid4

import sqlalchemy as sa
from fastapi import APIRouter, HTTPException, Request
from nous_sdk.contracts import (
    AdmitFiles,
    AdoptCognition,
    FreezeCatalog,
    ProcessCatalog,
    QueryRequest,
)
from pydantic import BaseModel, ConfigDict, Field

from .security import application, authorize, context

router = APIRouter(prefix="/api")


class NamespaceInput(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str = Field(min_length=1)
    operation_id: str = Field(default_factory=lambda: str(uuid4()))
    id: str = Field(default_factory=lambda: str(uuid4()))


class SubjectInput(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str = "default"
    capabilities: tuple[str, ...] = ("memory",)
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


@router.get("/namespaces")
async def namespaces(request: Request):
    from nous.identity.service import namespaces, subjects

    principal = await authorize(request)
    statement = sa.select(namespaces)
    if principal.namespace_ids is not None:
        statement = statement.where(namespaces.c.id.in_(principal.namespace_ids))
    if principal.subject_ids is not None:
        statement = statement.where(
            namespaces.c.id.in_(
                sa.select(subjects.c.namespace_id).where(subjects.c.id.in_(principal.subject_ids))
            )
        )
    async with application(request).authority.read() as view:
        return [dict(row) for row in (await view.connection.execute(statement)).mappings()]


@router.post("/namespaces")
async def create_namespace(request: Request, command: NamespaceInput):
    return await application(request).identity.create_namespace(
        await context(request, command.id), command.name, command.operation_id
    )


@router.get("/namespaces/{namespace_id}/subjects")
async def subjects(request: Request, namespace_id: str):
    from nous.identity.service import subjects

    principal = await authorize(request)
    if principal.namespace_ids is not None and namespace_id not in principal.namespace_ids:
        raise HTTPException(status_code=403, detail="Credential does not grant this namespace")
    statement = sa.select(subjects).where(subjects.c.namespace_id == namespace_id)
    if principal.subject_ids is not None:
        statement = statement.where(subjects.c.id.in_(principal.subject_ids))
    async with application(request).authority.read() as view:
        return [dict(row) for row in (await view.connection.execute(statement)).mappings()]


@router.post("/namespaces/{namespace_id}/subjects")
async def create_subject(request: Request, namespace_id: str, command: SubjectInput):
    return await application(request).identity.create_subject(
        await context(request, namespace_id),
        command.name,
        command.operation_id,
        capabilities=command.capabilities,
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/imports/catalogs")
async def freeze(request: Request, namespace_id: str, subject_id: str, command: FreezeCatalog):
    return await application(request).imports.freeze_catalog(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/imports/catalogs/{catalog_id}")
async def catalog(request: Request, namespace_id: str, subject_id: str, catalog_id: str):
    return await application(request).imports.read_catalog(
        await context(request, namespace_id, subject_id, capability="read"), catalog_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/imports/process")
async def process(request: Request, namespace_id: str, subject_id: str, command: ProcessCatalog):
    return await application(request).imports.submit_process(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/imports/files")
async def files(request: Request, namespace_id: str, subject_id: str, command: AdmitFiles):
    return await application(request).imports.admit_files(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/cognition")
async def adopt(request: Request, namespace_id: str, subject_id: str, command: AdoptCognition):
    return await application(request).cognition.adopt(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/query")
async def query(request: Request, namespace_id: str, subject_id: str, command: QueryRequest):
    return await application(request).retrieval.query(
        await context(request, namespace_id, subject_id, capability="read"), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/materials/{material_id}")
async def material(request: Request, namespace_id: str, subject_id: str, material_id: str):
    return await application(request).material.read(
        await context(request, namespace_id, subject_id, capability="read"), material_id
    )
