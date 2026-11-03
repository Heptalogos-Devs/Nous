"""One active host owns an instance; credentials and data outlive the process."""

import os
import secrets
from pathlib import Path
from types import TracebackType
from typing import Self

from filelock import FileLock, Timeout


class InstanceLock:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.lock = FileLock(root / "host.lock")

    def __enter__(self) -> Self:
        self.root.mkdir(parents=True, exist_ok=True)
        try:
            self.lock.acquire(timeout=0)
        except Timeout:
            raise RuntimeError("Instance already has an active host") from None
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.lock.release()


def initialize(root: Path) -> Path:
    for name in ("data", "content", "indexes", "credentials", "run"):
        (root / name).mkdir(parents=True, exist_ok=True)
    credentials = root / "credentials"
    credentials.chmod(0o700)
    token_file = credentials / "admin.token"
    if not token_file.exists():
        fd = os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as stream:
            stream.write(secrets.token_urlsafe(32))
    return token_file
