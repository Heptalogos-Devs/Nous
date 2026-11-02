"""One SDK lifetime and one presentation policy for all online commands."""

from __future__ import annotations

import asyncio
import json
import sys
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, cast

import anyio
import typer
from nous_sdk import NousClient
from nous_sdk.presentation import present as compact
from nous_sdk.transport import ClientOptions
from pydantic import BaseModel, JsonValue, TypeAdapter

from nous.platform.localization import Localizer

from .client_state import ClientState
from .render import present

if TYPE_CHECKING:
    from nous_sdk.namespace import NamespaceClient

_JSON = TypeAdapter(JsonValue)
_ENCODER = TypeAdapter(object)


@dataclass(frozen=True)
class Options:
    instance: Path
    url: str
    credential_file: Path | None
    locale: str
    json_mode: bool
    namespace: str | None = None
    raw: bool = False
    fields: tuple[str, ...] = ()
    client_file: Path | None = None
    client_state: ClientState | None = None


def options(ctx: typer.Context) -> Options:
    return cast(Options, ctx.obj)


def read_json(path: Path) -> JsonValue:
    raw = sys.stdin.read() if str(path) == "-" else path.read_text(encoding="utf-8")
    return _JSON.validate_json(raw)


def read_command[T: BaseModel](path: Path, model: type[T]) -> T:
    raw = sys.stdin.read() if str(path) == "-" else path.read_text(encoding="utf-8")
    return model.model_validate_json(raw)


async def download(
    api: NousClient, path: str, output: Path, *, params: Mapping[str, str] | None = None
) -> Path:
    """Stream an authorized response to a new file, removing interrupted output."""
    headers = await api.transport.headers()
    async with api.transport.http.stream("GET", path, headers=headers, params=params) as response:
        if not response.is_success:
            await api.transport.decode(response, headers["x-nous-trace-id"])
        created = False
        try:
            async with await anyio.open_file(output, "xb") as target:
                created = True
                async for chunk in response.aiter_bytes(256 * 1024):
                    await target.write(chunk)
        except BaseException:
            if created:
                with anyio.CancelScope(shield=True):
                    await anyio.Path(output).unlink(missing_ok=True)
            raise
    return output


def emit(ctx: typer.Context, value: object) -> None:
    selected = options(ctx)
    data = (
        _JSON.validate_python(_ENCODER.dump_python(value, mode="json"))
        if selected.raw
        else _JSON.validate_python(compact(value))
    )
    if selected.fields and isinstance(data, dict):
        data = {field: data[field] for field in selected.fields if field in data}
    if selected.json_mode:
        if isinstance(data, list):
            data = {"items": data, "count": len(data)}
        elif not isinstance(data, dict):
            data = {"value": data}
        sys.stdout.write(json.dumps(data, ensure_ascii=False, allow_nan=False) + "\n")
    else:
        present(data, Localizer(selected.locale))


def invoke(ctx: typer.Context, operation: Callable[[NousClient], Awaitable[object]]) -> None:
    selected = options(ctx)

    async def run() -> None:
        async with NousClient(
            selected.url,
            credential_file=selected.credential_file
            or selected.instance / "credentials" / "admin.token",
            options=ClientOptions(language=selected.locale),
        ) as root:
            emit(ctx, await operation(root))

    asyncio.run(run())


def invoke_bound(ctx: typer.Context, operation: Callable[[NamespaceClient], Awaitable[object]]) -> None:
    async def bound(root: NousClient) -> object:
        selected = options(ctx)
        if selected.namespace is None:
            raise ValueError("Choose a Namespace with 'nous use NAME' or --namespace NAME")
        namespace = await root.namespace(selected.namespace)
        return await operation(namespace)

    invoke(ctx, bound)
