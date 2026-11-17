"""Read client-selected files only when the CLI command actually needs them."""

import sys
from pathlib import Path

from nous.application.inputs import FileInput, InputProvider, StreamInput
from nous.common.errors import NousError
from nous.content.contracts import BodySpec, ExternalInput, InlineText


def body_binding(
    text: str | None, file: Path | None, *, key: str = "body"
) -> tuple[BodySpec, dict[str, InputProvider]]:
    """Declare caller-owned text without reading it before the application request."""
    if text is not None and file is not None:
        raise NousError("input.conflicting_body", "Choose one of --text, --file or stdin")
    if text is not None:
        return InlineText(text=text), {}
    if file is not None:
        return ExternalInput(key=key), {key: FileInput(file, name=file.name, source=str(file))}
    if sys.stdin.isatty():
        raise NousError("input.body_required", "Provide --text, --file or stdin")
    from .io import stdin_chunks

    return ExternalInput(key=key), {key: StreamInput(stdin_chunks(), name="stdin")}


def json_binding(path: Path, *, key: str) -> tuple[ExternalInput, dict[str, InputProvider]]:
    return ExternalInput(key=key), {key: FileInput(path, name=path.name, source=str(path))}
