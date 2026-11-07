"""运行配置中的命令，不把检查进程的 stdin 交给子进程。"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path

from settings import ROOT, CheckError, venv_python


def configure_stdio() -> None:
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            reconfigure(encoding="utf-8")


def emit(name: str, state: str, extra: str = "") -> None:
    suffix = f"  {extra}" if extra else ""
    print(f"{name:<8}{state}{suffix}")


def expand(argv: Sequence[str], path: str | None = None) -> list[str]:
    python = venv_python()
    result: list[str] = []
    for arg in argv:
        if "{venv_python}" in arg and not python.is_file():
            raise CheckError(f"找不到项目解释器：{python}")
        if path is not None:
            arg = arg.replace("{path}", path)
        result.append(arg.replace("{venv_python}", str(python)))
    return result


def executable(command: str) -> bool:
    return Path(command).is_file() or shutil.which(command) is not None


def run_argv(argv: Sequence[str], stdin: bytes | None = None) -> subprocess.CompletedProcess[bytes]:
    try:
        if stdin is None:
            return subprocess.run(
                list(argv),
                cwd=ROOT,
                capture_output=True,
                env=os.environ.copy(),
                stdin=subprocess.DEVNULL,
            )
        return subprocess.run(
            list(argv),
            cwd=ROOT,
            capture_output=True,
            env=os.environ.copy(),
            input=stdin,
        )
    except FileNotFoundError:
        message = f"找不到命令：{argv[0]}".encode()
        return subprocess.CompletedProcess(list(argv), 127, b"", message)


def command_text(result: subprocess.CompletedProcess[bytes]) -> str:
    text = (result.stdout + result.stderr).decode("utf-8", "replace").strip()
    if result.returncode != 0 and not text:
        return f"退出码 {result.returncode}"
    return text
