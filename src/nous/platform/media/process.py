"""Owned, bounded subprocess execution with cancellation reaching the process group."""

from __future__ import annotations

import asyncio
import os
import signal
from dataclasses import dataclass

from nous.platform.errors import Unavailable


class MediaProcessError(ValueError):
    pass


@dataclass(frozen=True)
class ProcessOutput:
    stdout: bytes
    stderr: bytes


async def _read_bounded(reader: asyncio.StreamReader, limit: int) -> bytes:
    blocks: list[bytes] = []
    length = 0
    while block := await reader.read(64 * 1024):
        length += len(block)
        if length > limit:
            raise MediaProcessError("Media process output exceeded its size limit")
        blocks.append(block)
    return b"".join(blocks)


async def _stop(process: asyncio.subprocess.Process) -> None:
    if process.returncode is not None:
        return
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
    except ProcessLookupError:
        pass
    try:
        async with asyncio.timeout(2):
            await process.wait()
    except TimeoutError:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            pass
        await process.wait()


async def run_process(
    arguments: tuple[str, ...], *, max_output_bytes: int = 4 * 1024 * 1024
) -> ProcessOutput:
    if not arguments:
        raise ValueError("A media executable is required")
    try:
        process = await asyncio.create_subprocess_exec(
            *arguments,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            start_new_session=os.name == "posix",
        )
    except FileNotFoundError as error:
        raise Unavailable(f"Media executable {arguments[0]!r} is not installed") from error
    if process.stdout is None or process.stderr is None:
        await _stop(process)
        raise RuntimeError("Media process pipes were not created")
    stdout = asyncio.create_task(_read_bounded(process.stdout, max_output_bytes))
    stderr = asyncio.create_task(_read_bounded(process.stderr, max_output_bytes))
    try:
        output, errors, result = await asyncio.gather(stdout, stderr, process.wait())
        if result != 0:
            raise MediaProcessError(
                f"Media process exited with status {result}: {errors[:4096].decode(errors='replace')}"
            )
        return ProcessOutput(output, errors)
    finally:
        # Reap before releasing the media workspace or its original-content lease.
        await _stop(process)
        for reader in (stdout, stderr):
            if not reader.done():
                reader.cancel()
        await asyncio.gather(stdout, stderr, return_exceptions=True)
