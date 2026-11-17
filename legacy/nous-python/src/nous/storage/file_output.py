"""Deliver complete saved bytes atomically to a caller-owned destination."""

import hashlib
import os
import tempfile
from collections.abc import Callable
from pathlib import Path

from nous.common.errors import NousError


def deliver_file(
    source: Path,
    destination: Path,
    *,
    digest: str,
    length: int,
    cancelled: Callable[[], bool] | None = None,
) -> Path:
    """Copy to the target filesystem, verify, and replace the destination atomically."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
    temporary = Path(name)
    try:
        calculated = hashlib.sha256()
        copied = 0
        with os.fdopen(descriptor, "wb") as output, source.open("rb") as input_file:
            while True:
                if cancelled is not None and cancelled():
                    raise InterruptedError("File delivery was cancelled")
                chunk = input_file.read(1024 * 1024)
                if not chunk:
                    break
                output.write(chunk)
                calculated.update(chunk)
                copied += len(chunk)
            output.flush()
            os.fsync(output.fileno())
        if copied != length or calculated.hexdigest() != digest:
            raise NousError("content.corrupt", "Exported bytes differ from the saved content")
        if cancelled is not None and cancelled():
            raise InterruptedError("File delivery was cancelled")
        os.replace(temporary, destination)
        return destination
    finally:
        temporary.unlink(missing_ok=True)
