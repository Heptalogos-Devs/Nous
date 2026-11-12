"""Local host command helpers shared by independent CLI scenarios."""

import subprocess
import sys
import time
from collections.abc import Callable
from pathlib import Path

from nous.runtime.bootstrap import initialize_instance


async def initialize(path: Path) -> None:
    await initialize_instance(path, environment={})


def wait_for(predicate: Callable[[], bool], *, timeout: float = 30) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError("Timed out waiting for the local host")
        time.sleep(0.05)


def command(
    instance: Path,
    *arguments: str,
    cwd: Path | None = None,
    data: str | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(instance),
            "--json",
            *arguments,
        ],
        cwd=cwd,
        input=data,
        text=True,
        encoding="utf-8",
        capture_output=True,
        timeout=45,
        env=env,
    )
