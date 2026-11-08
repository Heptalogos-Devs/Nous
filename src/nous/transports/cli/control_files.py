"""Perform client-side output I/O without blocking the instance host."""

from __future__ import annotations

import errno
import hashlib
import os
import shutil
import threading
from pathlib import Path


class ClientFiles:
    def __init__(self) -> None:
        self._prepared: dict[str, tuple[Path, Path, str, int]] = {}
        self._lock = threading.Lock()
        self._active: set[Path] = set()
        self._closed = False

    def perform(self, message: dict[str, object]) -> None:
        kind = message.get("kind")
        if kind == "write_text":
            path, text = message.get("target"), message.get("text")
            if not isinstance(path, str) or not isinstance(text, str):
                raise ValueError("Invalid local output request")
            target = Path(path)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text, encoding="utf-8")
            return
        key = message.get("key")
        if not isinstance(key, str):
            raise ValueError("Invalid local export identity")
        if kind == "export_commit":
            with self._lock:
                temporary, target, digest, length = self._prepared.pop(key)
                self._active.add(temporary)
            try:
                # A hard link publishes atomically when both locations share a
                # filesystem, without overwriting an existing destination.
                try:
                    os.link(temporary, target)
                except OSError as error:
                    if error.errno not in {
                        errno.EPERM,
                        errno.EACCES,
                        errno.EMLINK,
                        errno.EOPNOTSUPP,
                        errno.ENOSYS,
                        errno.EXDEV,
                        errno.EINVAL,
                    }:
                        raise
                    # Filesystems without hard links still preserve the previous
                    # exclusive-create and failed-copy cleanup semantics.
                    created = False
                    try:
                        with temporary.open("rb") as source, target.open("xb") as output:
                            created = True
                            shutil.copyfileobj(source, output, length=1024 * 1024)
                        with target.open("rb") as delivered:
                            if (
                                hashlib.file_digest(delivered, "sha256").hexdigest() != digest
                                or target.stat().st_size != length
                            ):
                                raise OSError("Exported bytes differ from the artifact identity")
                    except BaseException:
                        if created:
                            target.unlink(missing_ok=True)
                        raise
            finally:
                temporary.unlink(missing_ok=True)
                with self._lock:
                    self._active.discard(temporary)
            return
        source, destination = message.get("source"), message.get("target")
        temporary_name = message.get("temporary")
        digest, length = message.get("digest"), message.get("length")
        if (
            not isinstance(source, str)
            or not isinstance(destination, str)
            or not isinstance(temporary_name, str)
            or not Path(temporary_name).is_absolute()
            or not isinstance(digest, str)
            or not isinstance(length, int)
        ):
            raise ValueError("Invalid local export request")
        target = Path(destination)
        target.parent.mkdir(parents=True, exist_ok=True)
        temporary = Path(temporary_name)
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        # The enclosing session directory is private. Use the client's normal
        # creation mode so a same-filesystem hard link has ordinary export permissions.
        descriptor = os.open(temporary, flags, 0o666)
        try:
            calculated = hashlib.sha256()
            size = 0
            with os.fdopen(descriptor, "wb") as output:
                with self._lock:
                    if self._closed:
                        raise OSError("The client output session has ended")
                    self._active.add(temporary)
                with open(source, "rb") as input_file:
                    while chunk := input_file.read(1024 * 1024):
                        output.write(chunk)
                        calculated.update(chunk)
                        size += len(chunk)
                output.flush()
                os.fsync(output.fileno())
            if calculated.hexdigest() != digest or size != length:
                raise OSError("Exported bytes differ from the artifact identity")
            with self._lock:
                if self._closed:
                    raise OSError("The client output session has ended")
                self._prepared[key] = (temporary, target, digest, length)
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
        finally:
            with self._lock:
                self._active.discard(temporary)

    def cleanup(self) -> None:
        with self._lock:
            self._closed = True
            pending = [item[0] for item in self._prepared.values()]
            pending.extend(self._active)
            self._prepared.clear()
        for temporary in pending:
            try:
                temporary.unlink(missing_ok=True)
            except OSError:
                # Some platforms cannot unlink a file still being written; its
                # worker retries cleanup after closing it.
                pass
