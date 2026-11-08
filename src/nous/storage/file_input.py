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

from nous.runtime.execution import complete_cleanup

# A dedicated input thread detects parent death even while the main thread is
# blocked in open/read. Each process handles one request at a time and stays
# reusable only after a complete framed response has been consumed.
_READER = r"""
# NOUS_INGRESS_READER
import collections, json, os, queue, stat, struct, sys, threading
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
        requests.put((path, tid))

threading.Thread(target=receive, daemon=True).start()
while True:
    path, tid = requests.get()
    try:
        descriptor = os.open(caller_path(path, tid), os.O_RDONLY | getattr(os, "O_BINARY", 0))
        try:
            while True:
                block = os.read(descriptor, 1024 * 1024)
                if not block:
                    break
                send(struct.pack("!I", len(block)))
                send(block)
        finally:
            os.close(descriptor)
        send(struct.pack("!I", 0) + b"\x00")
    except OSError as error:
        detail = json.dumps({"errno": error.errno, "message": error.strerror}).encode()
        send(struct.pack("!I", 0) + b"\x01" + struct.pack("!I", len(detail)) + detail)
"""


async def _stop(process: asyncio.subprocess.Process) -> None:
    if process.returncode is None:
        try:
            process.kill()
        except ProcessLookupError:
            pass
    if process.stdin is not None:
        process.stdin.close()
    readers = [stream.read() for stream in (process.stdout, process.stderr) if stream is not None]
    await asyncio.gather(*readers, process.wait(), return_exceptions=True)


async def _launch() -> asyncio.subprocess.Process:
    launch = asyncio.create_task(
        asyncio.create_subprocess_exec(
            sys.executable,
            "-I",
            "-u",
            "-c",
            _READER,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
    )
    try:
        return await asyncio.shield(launch)
    except BaseException as primary:

        async def reap_late() -> None:
            try:
                process = await launch
            except Exception:
                return
            await _stop(process)

        try:
            await complete_cleanup(reap_late())
        except BaseException as cleanup:
            primary.add_note(f"File reader cleanup failed: {cleanup}")
        raise


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

    async def chunks(self, path: Path, chunk_size: int) -> AsyncGenerator[bytes]:
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
                await _stop(candidate)
            if process is None:
                process = await _launch()
            assert process.stdin is not None and process.stdout is not None
            encoded = os.fsencode(path)
            if len(encoded) > 1024 * 1024:
                raise ValueError("File path exceeds the reader limit")
            process.stdin.write(
                struct.pack("!I", len(encoded))
                + encoded
                + struct.pack("!Q", threading.get_native_id())
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
                        await complete_cleanup(_stop(process))
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
                await asyncio.gather(*(_stop(process) for process in idle))
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
