"""Client-owned terminal and file-output lifetimes."""

import asyncio
import os
import subprocess
import sys
from collections.abc import AsyncGenerator
from pathlib import Path

from nous.runtime.execution import acquire_owned, complete_cleanup, stop_process


async def stdin_chunks() -> AsyncGenerator[bytes]:
    """Read stdin through a cancellable process without buffering the whole input."""
    source = sys.stdin.buffer
    try:
        source.fileno()
    except OSError, ValueError:
        while chunk := source.read(1024 * 1024):
            yield chunk
        return

    program = """import os, sys
if os.name == 'nt':
    import msvcrt
    msvcrt.setmode(0, os.O_BINARY)
    msvcrt.setmode(1, os.O_BINARY)
while chunk := os.read(0, 65536):
    view = memoryview(chunk)
    while view:
        view = view[os.write(1, view):]
"""
    process = await acquire_owned(
        asyncio.create_subprocess_exec(
            sys.executable,
            "-c",
            program,
            stdin=sys.stdin,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        ),
        stop_process,
    )
    assert process.stdout is not None
    try:
        while chunk := await process.stdout.read(1024 * 1024):
            yield chunk
        if await process.wait():
            raise OSError("Could not read standard input")
    finally:
        if process.returncode is None:
            await complete_cleanup(stop_process(process))


async def read_stream() -> str:
    try:
        sys.stdin.fileno()
    except OSError, ValueError:
        return sys.stdin.read()
    program = """import os, sys, threading, time
def watch():
    while True:
        try:
            os.write(2, b'\\0')
        except OSError:
            os._exit(97)
        time.sleep(0.25)
threading.Thread(target=watch, daemon=True).start()
sys.stdout.buffer.write(sys.stdin.buffer.read())
"""
    process = await acquire_owned(
        asyncio.create_subprocess_exec(
            sys.executable,
            "-c",
            program,
            stdin=sys.stdin,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        ),
        stop_process,
    )
    try:
        data, _ = await process.communicate()
    except BaseException:
        if process.returncode is None:
            process.kill()
        await complete_cleanup(process.wait())
        raise
    return data.decode("utf-8")


async def write_text(path: Path, text: str) -> None:
    await write_bytes(path, text.encode("utf-8"))


async def write_bytes(path: Path, data: bytes) -> None:
    """Atomically deliver complete bytes into the destination directory."""
    program = """import os, pathlib, sys, tempfile, threading
size = int.from_bytes(sys.stdin.buffer.read(8), 'big')
data = sys.stdin.buffer.read(size)
def watch():
    if not os.read(0, 1):
        os._exit(97)
threading.Thread(target=watch, daemon=True).start()
path = pathlib.Path(sys.argv[1])
path.parent.mkdir(parents=True, exist_ok=True)
descriptor, temporary = tempfile.mkstemp(prefix='.' + path.name + '.', dir=path.parent)
try:
    with os.fdopen(descriptor, 'wb') as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
finally:
    pathlib.Path(temporary).unlink(missing_ok=True)
"""
    process = await acquire_owned(
        asyncio.create_subprocess_exec(
            sys.executable,
            "-c",
            program,
            str(path),
            stdin=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        ),
        stop_process,
    )
    assert process.stdin is not None and process.stderr is not None
    try:
        process.stdin.write(len(data).to_bytes(8, "big") + data)
        await process.stdin.drain()
        code, errors = await asyncio.gather(process.wait(), process.stderr.read())
    except BaseException:
        if process.returncode is None:
            process.kill()
        await complete_cleanup(process.wait())
        raise
    finally:
        process.stdin.close()
    if code:
        raise OSError(errors.decode("utf-8", errors="replace"))
