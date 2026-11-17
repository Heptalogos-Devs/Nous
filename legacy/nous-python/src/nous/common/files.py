"""Atomic publication of small text files."""

import os
import tempfile
from pathlib import Path


def write_text(target: Path, text: str) -> None:
    """Publish complete text beside its destination on the same filesystem."""
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".nous-", suffix=".tmp", dir=target.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)
    except BaseException as error:
        try:
            Path(temporary).unlink(missing_ok=True)
        except OSError as cleanup_error:
            error.add_note(
                f"Could not remove temporary configuration file {temporary}: {cleanup_error}"
            )
        raise
