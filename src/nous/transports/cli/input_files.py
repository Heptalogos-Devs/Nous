"""Read client-selected files only when the CLI command actually needs them."""

import sys
from pathlib import Path

from nous.application.inputs import InputParameter
from nous.common.errors import NousError
from nous.content.contracts import InputSpec


def body_parameter(
    field: str, text: str | None, file: Path | None, *, optional: bool = False
) -> InputParameter:
    if text is not None and file is not None:
        raise NousError("input.conflicting_body", "Choose one of --text, --file or stdin")
    source = (
        InputSpec(kind="text", text=text)
        if text is not None
        else InputSpec(kind="file", path=str(file))
        if file is not None
        else None
    )
    return InputParameter(
        field=tuple(field.split(".")),
        source=source,
        format="optional_body" if optional else "body",
        read_stdin=not sys.stdin.isatty(),
    )


def json_parameter(field: str, path: Path) -> InputParameter:
    return InputParameter(
        field=tuple(field.split(".")), source=InputSpec(kind="file", path=str(path)), format="json"
    )
