"""Cancelled delivery preserves the destination and cleans up its partial copy."""

import asyncio
import hashlib
import threading
from pathlib import Path
from unittest.mock import patch

import pytest

from nous.storage.file_output import deliver_file


def test_cancelled_delivery_preserves_destination(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.write_bytes(b"a" * (2 * 1024 * 1024))
    destination = tmp_path / "export"
    destination.write_bytes(b"existing")
    calls = 0

    def cancelled() -> bool:
        nonlocal calls
        calls += 1
        return calls == 2

    with pytest.raises(InterruptedError):
        deliver_file(
            source,
            destination,
            digest=hashlib.sha256(source.read_bytes()).hexdigest(),
            length=source.stat().st_size,
            cancelled=cancelled,
        )
    assert destination.read_bytes() == b"existing"
    assert {entry.name for entry in tmp_path.iterdir()} == {"source", "export"}


def test_local_export_waits_for_cancelled_copy(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import initialize_instance, open_instance

    destination = tmp_path / "export"
    destination.write_bytes(b"existing")

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        started, release = threading.Event(), threading.Event()

        def slow_copy(*args: object, **kwargs: object) -> Path:
            started.set()
            if not release.wait(10):
                raise TimeoutError("Copy was not released")
            return deliver_file(*args, **kwargs)  # type: ignore[arg-type]

        async with open_instance(instance, environment={}) as runtime:
            artifact = await runtime.artifacts.ingest(b"new content")
            with patch("nous.storage.content.deliver_file", new=slow_copy):
                task = asyncio.create_task(runtime.artifacts.export(artifact.ref, destination))
                assert await asyncio.to_thread(started.wait, 10)
                task.cancel()
                # The caller must remain alive until the borrowed file is no longer in use.
                await asyncio.sleep(0)
                assert not task.done()
                release.set()
                with pytest.raises(asyncio.CancelledError):
                    await task

    asyncio.run(scenario())
    assert destination.read_bytes() == b"existing"
    assert not list(tmp_path.glob(".export.*"))
