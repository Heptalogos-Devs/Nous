"""Derived media metadata and explicit adoption use the shared material owners."""

from hashlib import sha256

from fastapi import APIRouter, Request
from fastapi.responses import Response
from nous_wave.contracts import MediaReference, SourceRegion
from nous_wave.contracts.derivation import (
    CreateDerivedRegion,
    CreateRepresentation,
    DeriveMedia,
    RepresentationRef,
)

from .sdk_common import OperationIdentity
from .security import application, context

router = APIRouter()


class SourceRegionCreation(OperationIdentity):
    region: SourceRegion


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/representations")
async def adopt_representation(
    request: Request, namespace_id: str, subject_id: str, command: CreateRepresentation
):
    return await application(request).derivations.adopt(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/representations/read")
async def read_representation(
    request: Request, namespace_id: str, subject_id: str, command: RepresentationRef
):
    return await application(request).derivations.read(
        await context(request, namespace_id, subject_id, capability="read"), command
    )


@router.get(
    "/namespaces/{namespace_id}/subjects/{subject_id}/media/representations/{representation_id}/history"
)
async def representation_history(
    request: Request, namespace_id: str, subject_id: str, representation_id: str
):
    return await application(request).derivations.history(
        await context(request, namespace_id, subject_id, capability="read"), representation_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/derived-regions")
async def create_derived_region(
    request: Request, namespace_id: str, subject_id: str, command: CreateDerivedRegion
):
    return await application(request).derivations.register_region(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/media/derived-regions/{region_id}")
async def read_derived_region(request: Request, namespace_id: str, subject_id: str, region_id: str):
    return await application(request).derivations.read_region(
        await context(request, namespace_id, subject_id, capability="read"), region_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/source-regions")
async def create_source_region(
    request: Request, namespace_id: str, subject_id: str, command: SourceRegionCreation
):
    ctx = await context(request, namespace_id, subject_id)
    async with application(request).authority.write(
        ctx,
        command.operation_id,
        operation_kind="material.region",
        scope=subject_id,
        payload=command.region,
    ) as tx:
        if tx.replayed:
            saved = SourceRegion.model_validate(tx.replay_result)
            await application(request).material.read_region_in(tx, ctx, saved.id)
            return saved
        return tx.result(
            await application(request).material.register_region_in(tx, ctx, command.region)
        )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/lineage")
async def media_lineage(
    request: Request, namespace_id: str, subject_id: str, reference: MediaReference
):
    return await application(request).derivations.lineage(
        await context(request, namespace_id, subject_id, capability="read"), reference
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/media/{kind}/{reference_id}/content")
async def derived_content(
    request: Request, namespace_id: str, subject_id: str, kind: str, reference_id: str
):
    reference = MediaReference.model_validate({"kind": kind, "id": reference_id})
    if reference.kind not in {"representation", "derived_region"}:
        raise ValueError("Derived content requires an exact representation or derived region")
    value = await application(request).media_imports.derived_input(
        await context(request, namespace_id, subject_id, capability="read"), reference
    )
    return Response(
        value.data,
        media_type=value.media_type,
        headers={"x-nous-content-sha256": sha256(value.data).hexdigest()},
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/derive")
async def derive_media(request: Request, namespace_id: str, subject_id: str, command: DeriveMedia):
    return await application(request).media_imports.derive(
        await context(request, namespace_id, subject_id), command
    )
