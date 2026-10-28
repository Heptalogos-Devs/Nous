import base64
import inspect
import json
from collections.abc import AsyncIterable, Awaitable, Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

import anyio
import httpx
from pydantic import JsonValue, TypeAdapter

from .errors import NousError
from .source_scope import SourceReadScope
from .types import Download, JsonObject, QueryParam

TokenProvider = str | Callable[[], str | Awaitable[str]]


@dataclass(frozen=True)
class ClientOptions:
    timeout_seconds: float = 120.0
    connect_timeout_seconds: float = 10.0
    cleanup_timeout_seconds: float = 10.0
    max_response_bytes: int = 64 * 1024 * 1024
    chunk_bytes: int = 64 * 1024
    language: str | None = None
    proxy: str | None = None
    verify_tls: bool = True


class Transport:
    def __init__(self, url: str, token: TokenProvider, options: ClientOptions) -> None:
        endpoint = httpx.URL(url)
        if endpoint.scheme not in ("http", "https") or not endpoint.host or endpoint.userinfo:
            raise ValueError("Nous URL must be HTTP(S) without embedded credentials")
        if (
            min(
                options.timeout_seconds,
                options.connect_timeout_seconds,
                options.cleanup_timeout_seconds,
                options.max_response_bytes,
                options.chunk_bytes,
            )
            <= 0
        ):
            raise ValueError("client timeouts and byte limits must be positive")
        self.token = token
        self.options = options
        self.http = httpx.AsyncClient(
            base_url=str(endpoint).rstrip("/") + "/",
            timeout=httpx.Timeout(options.timeout_seconds, connect=options.connect_timeout_seconds),
            proxy=options.proxy,
            verify=options.verify_tls,
            trust_env=False,
        )

    async def close(self) -> None:
        await self.http.aclose()

    async def headers(
        self, trace_id: str | None = None, source_read: tuple[SourceReadScope, ...] = ()
    ) -> dict[str, str]:
        token = self.token() if callable(self.token) else self.token
        if inspect.isawaitable(token):
            token = await token
        if not token.strip():
            raise ValueError("Nous credential is empty")
        headers = {
            "Authorization": f"Bearer {token.strip()}",
            "x-nous-trace-id": trace_id or str(uuid4()),
        }
        if self.options.language:
            headers["Accept-Language"] = self.options.language
        if source_read:
            encoded = json.dumps(
                [scope.model_dump(mode="json") for scope in source_read],
                ensure_ascii=False,
                separators=(",", ":"),
            ).encode()
            headers["x-nous-source-read"] = base64.urlsafe_b64encode(encoded).decode().rstrip("=")
        return headers

    async def request(
        self,
        method: str,
        path: str,
        *,
        body: JsonValue = None,
        params: Mapping[str, QueryParam] | None = None,
        trace_id: str | None = None,
        source_read: tuple[SourceReadScope, ...] = (),
    ) -> JsonValue:
        headers = await self.headers(trace_id, source_read)
        try:
            async with self.http.stream(
                method, path, json=body, params=params, headers=headers
            ) as response:
                return await self.decode(response, headers["x-nous-trace-id"])
        except httpx.HTTPError as error:
            failure = NousError(
                {
                    "code": "DependencyUnavailable",
                    "message": str(error) or type(error).__name__,
                    "trace_id": headers["x-nous-trace-id"],
                }
            )
            if isinstance(body, Mapping) and "operation_id" in body:
                failure.context["operation_id"] = body["operation_id"]
            raise failure from error

    async def decode(self, response: httpx.Response, trace_id: str) -> JsonValue:
        trace_id = response.headers.get("x-nous-trace-id", trace_id)
        data = bytearray()
        async for chunk in response.aiter_bytes():
            if len(data) + len(chunk) > self.options.max_response_bytes:
                raise NousError(
                    {
                        "code": "ResourceExhausted",
                        "message": "response exceeds configured byte limit",
                        "trace_id": trace_id,
                    }
                )
            data.extend(chunk)
        try:
            value = TypeAdapter(JsonValue).validate_json(data)
        except (ValueError, UnicodeDecodeError) as error:
            raise NousError(
                {
                    "code": "DependencyUnavailable",
                    "message": "service returned invalid JSON",
                    "trace_id": trace_id,
                },
                status_code=response.status_code,
            ) from error
        if not response.is_success:
            try:
                envelope = TypeAdapter(JsonObject).validate_python(value)
                detail = TypeAdapter(JsonObject).validate_python(envelope.get("error", {}))
            except ValueError:
                detail: JsonObject = {}
            raise NousError(
                {
                    "message": f"service returned HTTP {response.status_code}",
                    "trace_id": trace_id,
                    **detail,
                },
                status_code=response.status_code,
            )
        return value

    async def upload(
        self,
        params: Mapping[str, QueryParam],
        content: AsyncIterable[bytes],
        media_type: str,
        length: int,
        source_read: tuple[SourceReadScope, ...] = (),
    ) -> JsonValue:
        headers = await self.headers(source_read=source_read)
        headers.update({"Content-Type": media_type, "Content-Length": str(length)})
        try:
            async with self.http.stream(
                "POST",
                "/api/media/materials",
                params=params,
                content=content,
                headers=headers,
            ) as response:
                return await self.decode(response, headers["x-nous-trace-id"])
        except httpx.HTTPError as error:
            raise NousError(
                {
                    "code": "DependencyUnavailable",
                    "message": str(error) or type(error).__name__,
                    "trace_id": headers["x-nous-trace-id"],
                }
            ) from error

    async def download(
        self,
        material_id: str,
        destination: Path,
        source_read: tuple[SourceReadScope, ...] = (),
        *,
        namespace_id: str,
        subject_id: str,
    ) -> Download:
        headers = await self.headers(source_read=source_read)
        async with self.http.stream(
            "GET",
            f"/api/namespaces/{namespace_id}/subjects/{subject_id}/materials/{material_id}/content",
            headers=headers,
        ) as response:
            if not response.is_success:
                await self.decode(response, headers["x-nous-trace-id"])
            length = 0
            output = await anyio.open_file(destination, "xb")
            try:
                async with output:
                    async for chunk in response.aiter_bytes(self.options.chunk_bytes):
                        await output.write(chunk)
                        length += len(chunk)
                    await output.flush()
            except BaseException:
                with anyio.CancelScope(shield=True):
                    await anyio.Path(destination).unlink(missing_ok=True)
                raise
            return Download(
                material_id=material_id,
                output=destination,
                byte_len=length,
                media_type=response.headers.get("content-type"),
                content_hash=response.headers.get("x-nous-content-sha256"),
            )


def token_file(path: str | Path) -> Callable[[], Awaitable[str]]:
    async def read() -> str:
        async with await anyio.open_file(path, "r", encoding="utf-8") as source:
            token = await source.read(4097)
        if len(token) > 4096:
            raise ValueError("credential file exceeds its supported size")
        return token.strip()

    return read
