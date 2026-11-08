"""Shared path and process-command primitives for temporary data management."""

from __future__ import annotations

import os
import subprocess
from collections.abc import Sequence
from pathlib import Path


class Unsafe(Exception):
    pass


def within(path: str | Path, parent: str | Path) -> bool:
    try:
        return os.path.normcase(
            os.path.commonpath((os.path.abspath(path), str(parent)))
        ) == os.path.normcase(str(parent))
    except ValueError:
        return False


def command(args: Sequence[str]) -> bytes:
    try:
        result = subprocess.run(args, capture_output=True, check=True, timeout=20)
        return result.stdout
    except (OSError, subprocess.SubprocessError):
        raise Unsafe(f"无法完成 {args[0]} 检查，保留候选。") from None
