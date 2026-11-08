"""Resolve declared command inputs within the owning application request."""

from __future__ import annotations

import json
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import TYPE_CHECKING, Literal, cast

from pydantic import BaseModel, ConfigDict

from nous.common.errors import NousError
from nous.content.contracts import InputSpec
from nous.content.input_access import current_input_access
from nous.storage.file_input import FileReaderPool

if TYPE_CHECKING:
    from nous.runtime.bootstrap import Runtime


class InputParameter(BaseModel):
    model_config = ConfigDict(extra="forbid")

    field: tuple[str, ...]
    source: InputSpec | None = None
    format: Literal["body", "optional_body", "json"]
    read_stdin: bool = False


async def read_input(runtime: Runtime | None, source: InputSpec) -> bytes:
    if not source.selection.is_whole:
        if runtime is None:
            raise NousError("input.invalid", "Text selection requires an open instance")
        return (await runtime.artifacts.read_transient_text(source)).encode("utf-8")
    if source.kind == "text":
        assert source.text is not None
        data = source.text.encode("utf-8")
        return data
    if source.kind == "artifact":
        if runtime is None:
            raise NousError("input.invalid", "This operation requires a file or text input")
        assert source.artifact is not None
        return await runtime.artifacts.read_bytes(source.artifact)
    assert source.path is not None
    path = Path(source.path)
    access = current_input_access.get()
    if access is not None:
        path = await access.materialize(path)
    if runtime is None:
        temporary = readers = FileReaderPool(1)
    else:
        temporary = None
        readers = runtime.artifacts.content.file_readers
    chunks = readers.chunks(path, 1024 * 1024)
    try:
        return b"".join([chunk async for chunk in chunks])
    finally:
        await chunks.aclose()
        if temporary is not None:
            await temporary.aclose()


async def bind_inputs(
    runtime: Runtime | None,
    kwargs: dict[str, object],
    inputs: tuple[InputParameter, ...],
    read_stream: Callable[[], Awaitable[str]] | None = None,
) -> dict[str, object]:
    result = dict(kwargs)
    for item in inputs:
        if not item.field:
            raise NousError("input.invalid", "An input parameter needs a field")
        if item.read_stdin and read_stream is None:
            raise NousError("input.unavailable", "Standard input is unavailable")
        incoming = await read_stream() if item.read_stdin and read_stream is not None else ""
        if item.format in {"body", "optional_body"}:
            if item.source is not None and incoming:
                raise NousError("input.conflicting_body", "Choose one of --text, --file or stdin")
            if item.source is None:
                if not item.read_stdin and item.format == "body":
                    raise NousError("input.body_required", "Provide --text, --file or stdin")
                value: object = incoming or None if item.format == "optional_body" else incoming
            else:
                value = (await read_input(runtime, item.source)).decode("utf-8")
        else:
            if item.source is None:
                raise NousError("input.invalid", "A JSON input needs a source")
            value = json.loads(await read_input(runtime, item.source))
        target = result
        for part in item.field[:-1]:
            child = target.get(part)
            if not isinstance(child, dict):
                raise NousError("input.invalid", "Input parameter field is invalid")
            target[part] = dict(cast(dict[str, object], child))
            target = cast(dict[str, object], target[part])
        target[item.field[-1]] = value
    return result
