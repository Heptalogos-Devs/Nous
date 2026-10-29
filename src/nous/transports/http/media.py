"""Stream immutable originals and route media formation through the import owner."""

from collections.abc import AsyncGenerator
from uuid import uuid4

import anyio
from fastapi import APIRouter, Request
from fastapi.responses import StreamingResponse
from nous_wave.contracts import (
    AdoptMedia,
    BlobDocument,
    MediaEmbeddingResult,
    MediaReference,
    TimePoint,
)

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
    event_time: str | None = None,
    expected_content_hash: str | None = None,
):
    app = application(request)
    ctx = await context(request, namespace_id, owner_subject_id)
    media_type = request.headers.get("content-type", "application/octet-stream")
    event = TimePoint.model_validate_json(event_time) if event_time else None
    lease = await app.content.publish(request.stream(), media_type)
    committed = False
    try:
        declared = request.headers.get("content-length")
        if declared is not None and int(declared) != lease.byte_len:
            raise ValueError("Uploaded byte length differs from Content-Length")
        if (
            expected_content_hash is not None
            and expected_content_hash.casefold() != lease.content_hash
        ):
            raise ValueError("Uploaded content does not match expected_content_hash")
        media = await app.media.freeze()
        info = await media.probe(lease)
        document = BlobDocument(filename=source, media_type=media_type, event_time=event)
        async with app.authority.write(
            ctx,
            operation_id or str(uuid4()),
            operation_kind="material.upload",
            scope=owner_subject_id,
            payload={"hash": lease.content_hash, "document": document.model_dump(mode="json")},
        ) as tx:
            if tx.replayed:
                assert tx.receipt is not None
                result = tx.receipt.result
                from pydantic import JsonValue, TypeAdapter

                retained = TypeAdapter(dict[str, JsonValue]).validate_python(result)
                record = TypeAdapter(dict[str, JsonValue]).validate_python(retained["result"])
                identifier = TypeAdapter(str).validate_python(record["id"])
                effective = (
                    (
                        await app.material.restore_input_in(
                            tx,
                            ctx,
                            (identifier,),
                            operation_id=operation_id or tx.operation_id,
                            operation_kind="material.upload",
                            scope=owner_subject_id,
                        )
                    ).context
                    if ctx.source_groups()
                    else ctx
                )
                await app.material.read_in(tx, effective, identifier)
            else:
                admission = await app.material.admit_blob_in(tx, ctx, lease, document, info)
                result = tx.result({"result": admission.material, "commit_seq": tx.commit_seq})
        committed = True
        return result
    finally:
        await app.content.release(lease)
        if not committed:
            await app.content.purge(ctx, (lease.content_hash,))


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
) -> MediaEmbeddingResult:
    return await application(request).retrieval.embed_media(
        await context(request, namespace_id, subject_id), reference, operation_id or str(uuid4())
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/materials/{material_id}/content")
async def download(request: Request, namespace_id: str, subject_id: str, material_id: str):
    app = application(request)
    value = await app.material.read(
        await context(request, namespace_id, subject_id, capability="read"), material_id
    )
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
