"""Authenticated ASGI access to the common application operations."""

import asyncio
import math
from collections.abc import AsyncGenerator
from contextlib import aclosing, asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING

from starlette.applications import Starlette
from starlette.requests import ClientDisconnect, Request
from starlette.responses import JSONResponse, Response, StreamingResponse
from starlette.routing import Route

from nous.application.inputs import StreamInput
from nous.common.errors import NousError
from nous.content.contracts import ExportOffer
from nous.presentation.json_selection import select_json
from nous.runtime.execution import complete_cleanup

from .asgi import BearerAuth, serve_asgi
from .shared import (
    CallRequest,
    ServiceAddress,
    call_events,
    content_chunks,
    error_value,
    input_bindings,
    json_bytes,
)

if TYPE_CHECKING:
    from starlette.types import ASGIApp

    from nous.application.dispatch import Application
    from nous.content.artifacts import ArtifactStore


async def _body(request: Request, maximum: int) -> bytes:
    data = bytearray()
    async for block in request.stream():
        data.extend(block)
        if len(data) > maximum:
            raise NousError("input.too_large", "Request exceeds the inline input limit")
    return bytes(data)


def _failure(error: Exception) -> JSONResponse:
    value = error_value(error)
    code = str(value["code"])
    status = (
        404
        if code.endswith("not_found") or code == "operation.unknown"
        else (
            504
            if code == "operation.timeout"
            else 409
            if "conflict" in code
            else 400
            if code.startswith(("input.", "material.", "content."))
            else 500
        )
    )
    details = value.get("details")
    if code == "model.request_failed" and isinstance(details, dict):
        status = 429 if details.get("status_code") == 429 else 502
    retry_after = value.get("retry_after")
    headers = (
        {"Retry-After": str(math.ceil(retry_after))}
        if isinstance(retry_after, (int, float))
        else None
    )
    return JSONResponse({"error": value}, status_code=status, headers=headers)


async def _call_until_disconnect(
    app: Application, payload: CallRequest, request: Request
) -> object:
    async def disconnected() -> None:
        while True:
            message = await request.receive()
            if message["type"] == "http.disconnect":
                return

    work = asyncio.create_task(
        app.call_name(
            payload.operation,
            payload.request,
            inputs=input_bindings(payload.inputs),
            timeout=payload.timeout,
            select=payload.select,
        )
    )
    monitor = asyncio.create_task(disconnected())
    try:
        done, _ = await asyncio.wait((work, monitor), return_when=asyncio.FIRST_COMPLETED)
        if work in done:
            return await work
        work.cancel()
        await complete_cleanup(asyncio.gather(work, return_exceptions=True))
        raise ClientDisconnect()
    finally:
        monitor.cancel()
        if not work.done():
            work.cancel()
        await complete_cleanup(asyncio.gather(work, monitor, return_exceptions=True))


async def _events(app: Application, payload: CallRequest) -> AsyncGenerator[bytes]:
    async with aclosing(call_events(app, payload)) as events:
        async for item in events:
            yield b"data: " + json_bytes(item) + b"\n\n"


def create_http_app(
    app: Application, artifacts: ArtifactStore, *, token: str, max_request_bytes: int = 8_388_608
) -> ASGIApp:
    if max_request_bytes < 1:
        raise ValueError("Request byte limit must be positive")

    async def operations(request: Request) -> Response:
        return JSONResponse({"operations": list(app.list())})

    async def describe(request: Request) -> Response:
        try:
            return JSONResponse(app.describe(request.path_params["name"]))
        except Exception as error:
            return _failure(error)

    async def call(request: Request) -> Response:
        try:
            payload = CallRequest.model_validate_json(await _body(request, max_request_bytes))
            value = await _call_until_disconnect(app, payload, request)
            return JSONResponse({"value": select_json(value)})
        except ClientDisconnect:
            raise
        except Exception as error:
            return _failure(error)

    async def stream(request: Request) -> Response:
        try:
            payload = CallRequest.model_validate_json(await _body(request, max_request_bytes))
        except Exception as error:
            return _failure(error)
        return StreamingResponse(_events(app, payload), media_type="text/event-stream")

    async def upload(request: Request) -> Response:
        try:
            name = request.query_params.get("name", "input")
            value = await app.call_name(
                "content.accept",
                {"key": "content", "name": name},
                inputs={"content": StreamInput(request.stream(), name=name)},
            )
            return JSONResponse({"value": select_json(value)})
        except ClientDisconnect:
            raise
        except Exception as error:
            return _failure(error)

    async def download(request: Request) -> Response:
        try:
            offer = ExportOffer.model_validate(
                await app.call_name("content.export_offer", {"ref": request.path_params["ref"]})
            )
            await artifacts.blob(offer.artifact_ref)
            return StreamingResponse(
                content_chunks(artifacts, offer.artifact_ref.identifier),
                media_type=offer.media_type,
            )
        except Exception as error:
            return _failure(error)

    routes = [
        Route("/operations", operations, methods=["GET"]),
        Route("/operations/{name:path}", describe, methods=["GET"]),
        Route("/calls", call, methods=["POST"]),
        Route("/calls/stream", stream, methods=["POST"]),
        Route("/content", upload, methods=["POST"]),
        Route("/content/{ref:path}", download, methods=["GET"]),
    ]
    return BearerAuth(Starlette(routes=routes), token)


@asynccontextmanager
async def serve_http(
    app: Application,
    artifacts: ArtifactStore,
    *,
    token: str,
    host: str = "127.0.0.1",
    port: int = 0,
    max_request_bytes: int = 8_388_608,
    tls_certfile: str | Path | None = None,
    tls_keyfile: str | Path | None = None,
) -> AsyncGenerator[ServiceAddress]:
    async with serve_asgi(
        create_http_app(app, artifacts, token=token, max_request_bytes=max_request_bytes),
        host=host,
        port=port,
        tls_certfile=tls_certfile,
        tls_keyfile=tls_keyfile,
    ) as address:
        yield address
