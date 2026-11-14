"""Common serialization, credentials and content delivery for remote transports."""

from __future__ import annotations

import asyncio
import base64
import binascii
import hmac
import ipaddress
import json
from collections.abc import AsyncGenerator
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from pydantic import BaseModel, ConfigDict, Field, JsonValue

from nous.application.inputs import InputProvider, StreamInput
from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import exception_context
from nous.presentation.json_selection import select_json
from nous.runtime.execution import complete_cleanup

if TYPE_CHECKING:
    from nous.application.contracts import EventEnvelope
    from nous.application.dispatch import Application
    from nous.content.artifacts import ArtifactStore


@dataclass(frozen=True)
class ServiceAddress:
    host: str
    port: int


def validate_listener(host: str, certfile: str | Path | None, keyfile: str | Path | None) -> None:
    if bool(certfile) != bool(keyfile):
        raise ValueError("TLS requires both a certificate and a private key")
    try:
        loopback = ipaddress.ip_address(host).is_loopback
    except ValueError:
        loopback = host.lower() == "localhost"
    if not loopback and not certfile:
        raise ValueError("A non-loopback listener requires TLS")


class InlineInput(BaseModel):
    model_config = ConfigDict(extra="forbid")
    data: str
    name: str = "input"
    source: str | None = None


class CallRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    operation: str = Field(min_length=1)
    request: JsonValue
    inputs: dict[str, InlineInput] = Field(default_factory=dict)
    timeout: float | None = Field(default=None, gt=0, allow_inf_nan=False)
    select: tuple[str, ...] = ()


def json_bytes(value: object) -> bytes:
    return json.dumps(
        select_json(value), ensure_ascii=False, separators=(",", ":"), allow_nan=False
    ).encode("utf-8")


def error_value(error: Exception) -> dict[str, JsonValue]:
    if isinstance(error, NousError):
        return error.to_dict()
    return NousError(
        failure_code(error), failure_message(error), details=exception_context(error)
    ).to_dict()


def authorized(header: str | None, token: str) -> bool:
    if not token:
        raise ValueError("Transport authentication token must be nonempty")
    scheme, separator, supplied = (header or "").partition(" ")
    return bool(
        separator
        and scheme.lower() == "bearer"
        and hmac.compare_digest(supplied.encode("utf-8"), token.encode("utf-8"))
    )


def input_bindings(values: dict[str, InlineInput]) -> dict[str, InputProvider]:
    result: dict[str, InputProvider] = {}
    for key, value in values.items():
        try:
            data = base64.b64decode(value.data, validate=True)
        except (ValueError, binascii.Error) as error:
            raise NousError(
                "input.invalid", "Input data must be valid base64", cause=error
            ) from error
        result[key] = StreamInput((data,), name=value.name, source=value.source)
    return result


async def call_events(
    app: Application,
    payload: CallRequest,
    *,
    timeout: float | None = None,  # noqa: ASYNC109
) -> AsyncGenerator[dict[str, JsonValue]]:
    """Keep ordered events and the terminal response within the caller's lifetime."""
    queue: asyncio.Queue[dict[str, JsonValue]] = asyncio.Queue(maxsize=16)

    async def observe(event: EventEnvelope) -> None:
        await queue.put({"kind": "event", "value": event.model_dump(mode="json")})

    async def run() -> None:
        try:
            value = await app.call_name(
                payload.operation,
                payload.request,
                inputs=input_bindings(payload.inputs),
                observer=observe,
                timeout=timeout if timeout is not None else payload.timeout,
                select=payload.select,
            )
            await queue.put({"kind": "result", "value": select_json(value)})
        except Exception as error:
            await queue.put({"kind": "error", "error": error_value(error)})

    work = asyncio.create_task(run(), name="nous-transport-call")
    try:
        while True:
            item = await queue.get()
            yield item
            if item["kind"] in {"result", "error"}:
                return
    finally:
        if not work.done():
            work.cancel()
        await complete_cleanup(asyncio.gather(work, return_exceptions=True))


async def content_chunks(artifacts: ArtifactStore, ref: str) -> AsyncGenerator[bytes]:
    """Stream a formally accepted artifact while the owning runtime is open."""
    blob = await artifacts.blob(ref)
    async with artifacts.content.open(blob) as reader:
        while chunk := await artifacts.content.execution.blocking(
            reader.read, artifacts.content.io_chunk_bytes
        ):
            yield chunk
