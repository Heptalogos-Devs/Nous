"""gRPC adapter using protobuf BytesValue envelopes with exact UTF-8 JSON payloads."""

from __future__ import annotations

import json
from collections.abc import AsyncGenerator, AsyncIterator
from contextlib import aclosing, asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING, NoReturn

import grpc
from google.protobuf.wrappers_pb2 import BytesValue
from pydantic import BaseModel, ConfigDict

from nous.application.inputs import StreamInput
from nous.common.errors import NousError
from nous.content.contracts import ExportOffer
from nous.runtime.execution import complete_cleanup

from .shared import (
    CallRequest,
    ServiceAddress,
    authorized,
    call_events,
    content_chunks,
    error_value,
    input_bindings,
    json_bytes,
    validate_listener,
)

if TYPE_CHECKING:
    from nous.application.dispatch import Application
    from nous.content.artifacts import ArtifactStore

SERVICE = "nous.transport.Application"


class UploadMetadata(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str = "input"
    source: str | None = None


class DownloadRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    ref: str


class DescribeRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str | None = None


def _message(value: object) -> BytesValue:
    return BytesValue(value=json_bytes(value))


async def _authenticate(
    context: grpc.aio.ServicerContext[BytesValue, BytesValue], token: str
) -> None:
    value = next(
        (
            value
            for key, value in context.invocation_metadata() or ()
            if key == "authorization" and isinstance(value, str)
        ),
        None,
    )
    if not authorized(value, token):
        await context.abort(grpc.StatusCode.UNAUTHENTICATED, "Authentication required")


async def _abort(
    context: grpc.aio.ServicerContext[BytesValue, BytesValue], error: Exception
) -> NoReturn:
    value = error_value(error)
    code = str(value["code"])
    status = (
        grpc.StatusCode.NOT_FOUND
        if code.endswith("not_found") or code == "operation.unknown"
        else grpc.StatusCode.DEADLINE_EXCEEDED
        if code == "operation.timeout"
        else grpc.StatusCode.FAILED_PRECONDITION
        if "conflict" in code
        else grpc.StatusCode.INVALID_ARGUMENT
        if code.startswith(("input.", "material.", "content."))
        else grpc.StatusCode.INTERNAL
    )
    details = value.get("details")
    if code == "model.request_failed" and isinstance(details, dict):
        status = (
            grpc.StatusCode.RESOURCE_EXHAUSTED
            if details.get("status_code") == 429
            else grpc.StatusCode.UNAVAILABLE
        )
    await context.abort(status, json.dumps(value, ensure_ascii=False))


def _timeout(
    payload: CallRequest, context: grpc.aio.ServicerContext[BytesValue, BytesValue]
) -> float | None:
    remaining = context.time_remaining()
    values = [value for value in (payload.timeout, remaining) if value is not None]
    return min(values) if values else None


def create_grpc_server(
    app: Application,
    artifacts: ArtifactStore,
    *,
    token: str,
    host: str = "127.0.0.1",
    port: int = 0,
    tls_certfile: str | Path | None = None,
    tls_keyfile: str | Path | None = None,
) -> tuple[grpc.aio.Server, ServiceAddress]:
    if not token:
        raise ValueError("Transport authentication token must be nonempty")
    validate_listener(host, tls_certfile, tls_keyfile)

    async def describe(
        request: BytesValue, context: grpc.aio.ServicerContext[BytesValue, BytesValue]
    ) -> BytesValue:
        await _authenticate(context, token)
        try:
            query = DescribeRequest.model_validate_json(request.value)
            value = app.describe(query.name) if query.name else {"operations": list(app.list())}
            return _message({"value": value})
        except Exception as error:
            await _abort(context, error)

    async def call(
        request: BytesValue, context: grpc.aio.ServicerContext[BytesValue, BytesValue]
    ) -> BytesValue:
        await _authenticate(context, token)
        try:
            payload = CallRequest.model_validate_json(request.value)
            value = await app.call_name(
                payload.operation,
                payload.request,
                inputs=input_bindings(payload.inputs),
                timeout=_timeout(payload, context),
                select=payload.select,
            )
            return _message({"value": value})
        except Exception as error:
            await _abort(context, error)

    async def stream_call(
        request: BytesValue, context: grpc.aio.ServicerContext[BytesValue, BytesValue]
    ) -> AsyncGenerator[BytesValue]:
        await _authenticate(context, token)
        try:
            payload = CallRequest.model_validate_json(request.value)
            async with aclosing(
                call_events(app, payload, timeout=_timeout(payload, context))
            ) as events:
                async for item in events:
                    yield _message(item)
        except Exception as error:
            await _abort(context, error)

    async def upload(
        requests: AsyncIterator[BytesValue],
        context: grpc.aio.ServicerContext[BytesValue, BytesValue],
    ) -> BytesValue:
        await _authenticate(context, token)
        try:
            first = await anext(requests)
            metadata = UploadMetadata.model_validate_json(first.value)

            async def chunks() -> AsyncGenerator[bytes]:
                async for part in requests:
                    yield part.value

            value = await app.call_name(
                "content.accept",
                {"key": "content", "name": metadata.name},
                inputs={
                    "content": StreamInput(chunks(), name=metadata.name, source=metadata.source)
                },
                timeout=context.time_remaining(),
            )
            return _message({"value": value})
        except StopAsyncIteration:
            await _abort(context, NousError("input.invalid", "Upload metadata is required"))
        except Exception as error:
            await _abort(context, error)

    async def download(
        request: BytesValue, context: grpc.aio.ServicerContext[BytesValue, BytesValue]
    ) -> AsyncGenerator[BytesValue]:
        await _authenticate(context, token)
        try:
            query = DownloadRequest.model_validate_json(request.value)
            offer = ExportOffer.model_validate(
                await app.call_name("content.export_offer", {"ref": query.ref})
            )
            blob = await artifacts.blob(offer.artifact_ref)
            yield _message(
                {
                    "artifact": offer.artifact_ref,
                    "name": offer.suggested_name,
                    "media_type": offer.media_type,
                    "byte_length": blob.byte_length,
                }
            )
            async for block in content_chunks(artifacts, offer.artifact_ref.identifier):
                yield BytesValue(value=block)
        except Exception as error:
            await _abort(context, error)

    server = grpc.aio.server()
    handlers = {
        "Describe": grpc.unary_unary_rpc_method_handler(
            describe,
            request_deserializer=BytesValue.FromString,
            response_serializer=BytesValue.SerializeToString,
        ),
        "Call": grpc.unary_unary_rpc_method_handler(
            call,
            request_deserializer=BytesValue.FromString,
            response_serializer=BytesValue.SerializeToString,
        ),
        "StreamCall": grpc.unary_stream_rpc_method_handler(
            stream_call,
            request_deserializer=BytesValue.FromString,
            response_serializer=BytesValue.SerializeToString,
        ),
        "Upload": grpc.stream_unary_rpc_method_handler(
            upload,
            request_deserializer=BytesValue.FromString,
            response_serializer=BytesValue.SerializeToString,
        ),
        "Download": grpc.unary_stream_rpc_method_handler(
            download,
            request_deserializer=BytesValue.FromString,
            response_serializer=BytesValue.SerializeToString,
        ),
    }
    server.add_generic_rpc_handlers((grpc.method_handlers_generic_handler(SERVICE, handlers),))
    address = f"[{host}]:{port}" if ":" in host else f"{host}:{port}"
    if tls_certfile and tls_keyfile:
        credentials = grpc.ssl_server_credentials(
            [(Path(tls_keyfile).read_bytes(), Path(tls_certfile).read_bytes())]
        )
        actual_port = server.add_secure_port(address, credentials)
    else:
        actual_port = server.add_insecure_port(address)
    if not actual_port:
        raise OSError("Cannot bind gRPC server")
    return server, ServiceAddress(host=host, port=actual_port)


@asynccontextmanager
async def serve_grpc(
    app: Application,
    artifacts: ArtifactStore,
    *,
    token: str,
    host: str = "127.0.0.1",
    port: int = 0,
    shutdown_grace: float = 30,
    tls_certfile: str | Path | None = None,
    tls_keyfile: str | Path | None = None,
) -> AsyncGenerator[ServiceAddress]:
    server, address = create_grpc_server(
        app,
        artifacts,
        token=token,
        host=host,
        port=port,
        tls_certfile=tls_certfile,
        tls_keyfile=tls_keyfile,
    )
    await server.start()
    try:
        yield address
    finally:
        await complete_cleanup(server.stop(shutdown_grace))
