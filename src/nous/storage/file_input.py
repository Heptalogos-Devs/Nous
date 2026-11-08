"""Bounded reusable processes for cancellable caller-selected file reads."""

from __future__ import annotations

import asyncio
import json
import os
import struct
import sys
import threading
from collections.abc import AsyncGenerator
from pathlib import Path
from typing import cast

from nous.common.errors import NousError
from nous.content.contracts import SourceSelection
from nous.runtime.execution import acquire_owned, complete_cleanup, stop_process

_SOURCE_ROOT = Path(__file__).resolve().parents[2]

# A dedicated input thread detects parent death even while the main thread is
# blocked in open/read. Each process handles one request at a time and stays
# reusable only after a complete framed response has been consumed.
_READER = r"""
# NOUS_INGRESS_READER
import collections, json, os, queue, stat, struct, sys, threading
sys.path.insert(0, sys.argv[1])
from nous.common.errors import NousError
from nous.content.contracts import SourceSelection
from nous.content.text_selection import TextSelection, read_seekable
if os.name == "nt":
    import msvcrt
    for descriptor in (0, 1, 2):
        msvcrt.setmode(descriptor, os.O_BINARY)

def exact(size):
    data = bytearray()
    while len(data) < size:
        part = os.read(0, size - len(data))
        if not part:
            os._exit(97)
        data.extend(part)
    return bytes(data)

def send(data):
    view = memoryview(data)
    while view:
        view = view[os.write(1, view):]

def caller_path(path, tid):
    # Linux procfs resolves self/thread-self in the process opening the file.
    # Walk symlink prefixes to preserve the caller's identity, including when
    # a user-selected symlink points into procfs. Keep the remaining suffix for
    # the kernel to resolve (notably procfs magic links).
    if not sys.platform.startswith("linux"):
        return path
    remaining = collections.deque(path.split("/"))
    resolved = []
    links = 0
    while remaining:
        part = remaining.popleft()
        if not part or part == ".":
            continue
        if part == "..":
            if resolved:
                resolved.pop()
            continue
        resolved.append(part)
        if resolved in (["proc", "self"], ["proc", "thread-self"]):
            try:
                is_procfs = os.readlink("/proc/self") == str(os.getpid())
            except OSError:
                is_procfs = False
            if is_procfs:
                base = "/proc/" + str(os.getppid())
                if part == "thread-self":
                    base += "/task/" + str(tid)
                return base + ("/" + "/".join(remaining) if remaining else "")
        current = "/" + "/".join(resolved)
        try:
            mode = os.lstat(current).st_mode
            if not stat.S_ISLNK(mode):
                if remaining and not stat.S_ISDIR(mode):
                    return path
                continue
            target = os.readlink(current)
        except OSError:
            return path
        links += 1
        if links > 40:
            return path
        if target.startswith("/"):
            resolved.clear()
        else:
            resolved.pop()
        remaining.extendleft(reversed(target.split("/")))
    return path

requests = queue.SimpleQueue()
def receive():
    while True:
        size = struct.unpack("!I", exact(4))[0]
        if size > 1024 * 1024:
            os._exit(98)
        path = os.fsdecode(exact(size))
        tid = struct.unpack("!Q", exact(8))[0]
        config_size = struct.unpack("!I", exact(4))[0]
        if config_size > 4096:
            os._exit(98)
        config = json.loads(exact(config_size))
        requests.put((path, tid, config))

threading.Thread(target=receive, daemon=True).start()
while True:
    path, tid, config = requests.get()
    try:
        descriptor = os.open(caller_path(path, tid), os.O_RDONLY | getattr(os, "O_BINARY", 0))
        try:
            if config is not None:
                chosen = SourceSelection.model_validate(config["selection"])
                info = os.fstat(descriptor)
                if stat.S_ISREG(info.st_mode) and info.st_size > 0:
                    data = read_seekable(descriptor, chosen, config["limit"])[0]
                else:
                    selection = TextSelection(chosen, config["limit"])
                    while not selection.done:
                        block = os.read(descriptor, selection.next_read_size())
                        if not block:
                            break
                        selection.feed(block)
                    data = selection.finish()[0]
                for start in range(0, len(data), 1024 * 1024):
                    block = data[start:start + 1024 * 1024]
                    send(struct.pack("!I", len(block)))
                    send(block)
            else:
                while True:
                    block = os.read(descriptor, 1024 * 1024)
                    if not block:
                        break
                    send(struct.pack("!I", len(block)))
                    send(block)
        finally:
            os.close(descriptor)
        send(struct.pack("!I", 0) + b"\x00")
    except (OSError, NousError) as error:
        detail = json.dumps({"errno": getattr(error, "errno", None), "message": str(error), "code": getattr(error, "code", None)}).encode()
        send(struct.pack("!I", 0) + b"\x01" + struct.pack("!I", len(detail)) + detail)
"""


async def _launch() -> asyncio.subprocess.Process:
    return await acquire_owned(
        asyncio.create_subprocess_exec(
            sys.executable,
            "-I",
            "-u",
            "-c",
            _READER,
            str(_SOURCE_ROOT),
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        ),
        stop_process,
    )


class FileReaderPool:
    """Reuse bounded readers; cancellation destroys only the affected process."""

    def __init__(self, limit: int) -> None:
        if limit < 1:
            raise ValueError("File reader limit must be positive")
        self._limit = limit
        self._slots = asyncio.Semaphore(limit)
        self._close_lock = asyncio.Lock()
        self._idle: list[asyncio.subprocess.Process] = []
        self._closed = False
        self._drained = False

    async def chunks(
        self,
        path: Path,
        chunk_size: int,
        *,
        selection: SourceSelection | None = None,
        limit: int | None = None,
    ) -> AsyncGenerator[bytes]:
        if chunk_size < 1:
            raise ValueError("File read size must be positive")
        # A reusable child retains its launch directory. Anchor relative paths
        # in the caller before queueing, without collapsing symlink/.. segments.
        if not path.is_absolute():
            path = Path.cwd() / path
        await self._slots.acquire()
        process: asyncio.subprocess.Process | None = None
        reusable = False
        try:
            if self._closed:
                raise RuntimeError("File reader pool is closed")
            while self._idle:
                candidate = self._idle.pop()
                if candidate.returncode is None:
                    process = candidate
                    break
                await stop_process(candidate)
            if process is None:
                process = await _launch()
            assert process.stdin is not None and process.stdout is not None
            encoded = os.fsencode(path)
            if len(encoded) > 1024 * 1024:
                raise ValueError("File path exceeds the reader limit")
            config = json.dumps(
                {"selection": selection.model_dump(mode="json"), "limit": limit}
                if selection is not None
                else None
            ).encode()
            if selection is not None and limit is None:
                raise ValueError("Selected input requires a byte limit")
            process.stdin.write(
                struct.pack("!I", len(encoded))
                + encoded
                + struct.pack("!Q", threading.get_native_id())
                + struct.pack("!I", len(config))
                + config
            )
            await process.stdin.drain()
            while True:
                size = struct.unpack("!I", await process.stdout.readexactly(4))[0]
                if size > 1024 * 1024:
                    raise OSError("Invalid file reader response")
                if size == 0:
                    status = await process.stdout.readexactly(1)
                    if status == b"\x00":
                        reusable = True
                        break
                    if status != b"\x01":
                        raise OSError("Invalid file reader status")
                    length = struct.unpack("!I", await process.stdout.readexactly(4))[0]
                    if length > 4096:
                        raise OSError("Invalid file reader error")
                    raw = await process.stdout.readexactly(length)
                    try:
                        details = cast(dict[str, object], json.loads(raw))
                    except ValueError, TypeError:
                        details = {}
                    number, message = details.get("errno"), details.get("message")
                    if isinstance(details.get("code"), str):
                        raise NousError(str(details["code"]), str(message))
                    raise OSError(
                        number if isinstance(number, int) else None,
                        message if isinstance(message, str) else "File reader failed",
                        os.fspath(path),
                    )
                remaining = size
                while remaining:
                    block = await process.stdout.read(min(remaining, chunk_size))
                    if not block:
                        raise OSError("File reader terminated unexpectedly")
                    remaining -= len(block)
                    yield block
        except asyncio.IncompleteReadError as error:
            raise OSError("File reader terminated unexpectedly") from error
        finally:
            try:
                if process is not None:
                    if reusable and not self._closed and process.returncode is None:
                        self._idle.append(process)
                    else:
                        await complete_cleanup(stop_process(process))
            finally:
                self._slots.release()

    async def aclose(self) -> None:
        """Stop admission, wait for borrowed readers, then reap all idle processes."""
        await complete_cleanup(self._close())

    async def _close(self) -> None:
        async with self._close_lock:
            if self._drained:
                return
            self._closed = True
            acquired = 0
            try:
                for _ in range(self._limit):
                    await self._slots.acquire()
                    acquired += 1
                idle, self._idle = self._idle, []
                await asyncio.gather(*(stop_process(process) for process in idle))
                self._drained = True
            finally:
                for _ in range(acquired):
                    self._slots.release()


async def file_chunks(path: Path, chunk_size: int) -> AsyncGenerator[bytes]:
    """Use a temporary one-reader pool for callers without a runtime owner."""
    pool = FileReaderPool(1)
    stream = pool.chunks(path, chunk_size)
    try:
        async for block in stream:
            yield block
    finally:
        await stream.aclose()
        await pool.aclose()
