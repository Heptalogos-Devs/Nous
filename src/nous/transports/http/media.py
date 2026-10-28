"""Stream immutable originals and route media formation through the import owner."""

from collections.abc import AsyncGenerator
from uuid import uuid4

import anyio
from fastapi import APIRouter, Request
from fastapi.responses import StreamingResponse
from nous_wave.contracts import AdoptMedia, BlobDocument, MediaReference

from nous.platform.content import ContentObject

from .security import application, context

router = APIRouter(prefix="/api")


@router.post("/media/materials")
async def upload(
    request: Request,
    namespace_id: str,
    owner_subject_id: str,
    source: str = "media",
    operation_id: str | None = None,
):
    app = application(request)
    ctx = await context(request, namespace_id, owner_subject_id)
    media_type = request.headers.get("content-type", "application/octet-stream")
    lease = await app.content.publish(request.stream(), media_type)
    try:
        info = await app.media.probe(lease)
        document = BlobDocument(filename=source, media_type=media_type)
        async with app.authority.write(
            ctx,
            operation_id or str(uuid4()),
            operation_kind="material.upload",
            scope=owner_subject_id,
            payload={"hash": lease.content_hash, "document": document.model_dump(mode="json")},
        ) as tx:
            if tx.replayed:
                assert tx.receipt is not None
                return tx.receipt.result
            value = await app.material.put_blob_in(tx, ctx, lease, document, info)
            await app.material.observe_in(tx, ctx, str(value["id"]), document)
            return tx.result({"result": value, "commit_seq": tx.commit_seq})
    finally:
        await app.content.release(lease)


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/adopt")
async def adopt_media(request: Request, namespace_id: str, subject_id: str, command: AdoptMedia):
    return await application(request).media_imports.adopt(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/media/embed")
async def embed_media(
    request: Request,
    namespace_id: str,
    subject_id: str,
    reference: MediaReference,
    operation_id: str | None = None,
):
    return await application(request).retrieval.embed_media(
        await context(request, namespace_id, subject_id), reference, operation_id or str(uuid4())
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/materials/{material_id}/content")
async def download(request: Request, namespace_id: str, subject_id: str, material_id: str):
    app = application(request)
    value = await app.material.read(await context(request, namespace_id, subject_id), material_id)
    obj = ContentObject(value["content_hash"], value["byte_len"], value["media_type"])

    async def chunks() -> AsyncGenerator[bytes]:
        async with app.content.protect(obj) as path, await anyio.open_file(path, "rb") as source:
            while chunk := await source.read(65536):
                yield chunk

    return StreamingResponse(
        chunks(),
        media_type=obj.media_type,
        headers={"Content-Length": str(obj.byte_len), "x-nous-content-sha256": obj.content_hash},
    )
