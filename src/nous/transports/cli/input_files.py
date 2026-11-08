"""Read client-selected files only when the CLI command actually needs them."""

from pathlib import Path

from nous.content.input_access import current_input_access


def _local(path: Path) -> Path:
    access = current_input_access.get()
    return access.materialize_sync(path) if access is not None else path


def input_bytes(path: Path) -> bytes:
    return _local(path).read_bytes()


def input_text(path: Path) -> str:
    return _local(path).read_text(encoding="utf-8")
