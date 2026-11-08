"""Runtime ownership and exported content retain their actual instance identity."""

import asyncio
import json
import logging
import shutil
import threading
from contextlib import AsyncExitStack
from pathlib import Path
from typing import BinaryIO
from unittest.mock import patch

import pytest

from nous.common.errors import NousError
from nous.observability.instrumentation import active_providers, get_logger
from nous.runtime.bootstrap import Runtime, initialize_instance, open_instance


def test_cancellation_during_content_creation_releases_temporary_file(tmp_path: Path) -> None:
    from nous.storage import content

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        created, release = threading.Event(), threading.Event()
        files: list[Path] = []
        original = content.tempfile.mkstemp

        def delayed(*, dir: Path, suffix: str) -> tuple[int, str]:
            descriptor, name = original(dir=dir, suffix=suffix)
            files.append(Path(name))
            created.set()
            assert release.wait(10)
            return descriptor, name

        async with open_instance(instance, environment={}) as runtime:
            with patch("nous.storage.content.tempfile.mkstemp", new=delayed):
                task = asyncio.create_task(runtime.artifacts.ingest("content"))
                try:
                    assert await asyncio.wait_for(asyncio.to_thread(created.wait), 10)
                    task.cancel()
                finally:
                    release.set()
                with pytest.raises(asyncio.CancelledError):
                    await task
            assert len(files) == 1 and not files[0].exists()

    asyncio.run(scenario())


def test_cancellation_during_frame_workspace_creation_removes_directory(tmp_path: Path) -> None:
    import sys
    import tempfile

    from nous.media.audiovisual import AudioVisual

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        created, release = threading.Event(), threading.Event()
        directories: list[Path] = []
        original = tempfile.TemporaryDirectory

        def delayed(*, prefix: str, dir: Path) -> tempfile.TemporaryDirectory[str]:
            directory = original(prefix=prefix, dir=dir)
            directories.append(Path(directory.name))
            created.set()
            assert release.wait(10)
            return directory

        async with open_instance(instance, environment={}) as runtime:
            settings = runtime.configuration.snapshot.values.media.model_copy(
                update={"ffmpeg_path": sys.executable, "ffprobe_path": sys.executable}
            )
            media = AudioVisual(settings, runtime.execution)

            async def read_frames() -> None:
                async with media.frames(tmp_path / "video", (0,), 64, tmp_path / "frames"):
                    pass

            with patch("nous.media.audiovisual.tempfile.TemporaryDirectory", new=delayed):
                task = asyncio.create_task(read_frames())
                try:
                    assert await asyncio.wait_for(asyncio.to_thread(created.wait), 10)
                    task.cancel()
                finally:
                    release.set()
                with pytest.raises(asyncio.CancelledError):
                    await task
            assert len(directories) == 1 and not directories[0].exists()

    asyncio.run(scenario())


def test_spawn_captures_new_configuration_outside_parent_request(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            async with runtime.execution.request():
                original = runtime.execution.snapshot
                await runtime.configuration.update({"logging.level": "DEBUG"})
                current = runtime.configuration.snapshot
                assert current.version > original.version

                async def capture() -> int:
                    return runtime.execution.snapshot.version

                task = runtime.execution.spawn("background.check", capture)
                assert await task == current.version

                async def durable_capture() -> int:
                    async with runtime.execution.durable():
                        async with runtime.execution.operation("durable.check") as durable:
                            return durable.settings.version

                assert await asyncio.create_task(durable_capture()) == current.version
                assert runtime.execution.snapshot.version == original.version

    asyncio.run(scenario())


def test_instance_logs_follow_operations_in_existing_workers_and_interleaved_close(
    tmp_path: Path,
) -> None:
    import structlog

    async def scenario() -> None:
        first_path, second_path = tmp_path / "first", tmp_path / "second"
        root = logging.getLogger()
        process_logging = (root.level, tuple(root.handlers), dict(structlog.get_config()))
        work: asyncio.Queue[tuple[Runtime, str, asyncio.Event] | None] = asyncio.Queue()

        async def worker() -> None:
            while item := await work.get():
                runtime, label, done = item
                try:
                    async with runtime.execution.operation(label):
                        get_logger("nous.test.worker").warning(label)
                        if label == "first.failed":
                            raise NousError("input.failed", "first-only-error")
                except NousError:
                    pass
                done.set()

        task = asyncio.create_task(worker())
        first_stack, second_stack = AsyncExitStack(), AsyncExitStack()
        try:
            await initialize_instance(first_path, environment={})
            await initialize_instance(second_path, environment={})
            first = await first_stack.enter_async_context(open_instance(first_path, environment={}))
            second = await second_stack.enter_async_context(
                open_instance(second_path, environment={})
            )
            for runtime, label in ((first, "first.failed"), (second, "second.before")):
                done = asyncio.Event()
                await work.put((runtime, label, done))
                await asyncio.wait_for(done.wait(), 3)
            root.warning("host-only-event")
            await asyncio.create_task(first_stack.aclose())
            done = asyncio.Event()
            await work.put((second, "second.after", done))
            await asyncio.wait_for(done.wait(), 3)
            await asyncio.create_task(second_stack.aclose())
            assert active_providers.get() is None
        finally:
            await work.put(None)
            await task
            await second_stack.aclose()
            await first_stack.aclose()
        assert (root.level, tuple(root.handlers), dict(structlog.get_config())) == process_logging
        first_events = [
            json.loads(line) for line in (first_path / "logs/nous.jsonl").read_text().splitlines()
        ]
        second_events = [
            json.loads(line) for line in (second_path / "logs/nous.jsonl").read_text().splitlines()
        ]
        assert any(event.get("error_message") == "first-only-error" for event in first_events)
        assert any(event.get("event") == "second.after" for event in second_events)
        assert all(
            event.get("instance_id") == first.descriptor.instance_id for event in first_events
        )
        assert all(
            event.get("instance_id") == second.descriptor.instance_id for event in second_events
        )
        assert not any("first-only-error" in json.dumps(event) for event in second_events)
        assert not any(
            event.get("event") == "host-only-event" for event in first_events + second_events
        )

    asyncio.run(scenario())


def test_nested_instances_own_snapshots_capacity_and_shutdown(tmp_path: Path) -> None:
    async def scenario() -> None:
        first_path, second_path = tmp_path / "first", tmp_path / "second"
        await initialize_instance(first_path, environment={})
        await initialize_instance(second_path, environment={})
        async with (
            open_instance(
                first_path, environment={}, overrides={"execution.max_in_flight": 1}
            ) as first,
            AsyncExitStack() as second_stack,
        ):
            second = await second_stack.enter_async_context(
                open_instance(second_path, environment={}, overrides={"execution.max_in_flight": 1})
            )
            occupied, release_occupied = asyncio.Event(), asyncio.Event()
            queued, entered, release_cross = asyncio.Event(), asyncio.Event(), asyncio.Event()
            left_second, release_first_tail = asyncio.Event(), asyncio.Event()
            closing_started = asyncio.Event()
            first_snapshot = first.configuration.snapshot
            second_snapshot = second.configuration.snapshot

            async def occupy_second() -> None:
                async with second.execution.operation("second.occupy"):
                    occupied.set()
                    await release_occupied.wait()

            async def cross_instance() -> None:
                async with first.execution.operation("first.outer") as outer:
                    assert outer.settings is first_snapshot
                    assert second.execution.snapshot is second_snapshot
                    await first.configuration.update({"runtime.shutdown_grace_seconds": 2})
                    assert outer.settings is first_snapshot
                    queued.set()
                    async with second.execution.operation("second.cross") as crossing:
                        assert crossing.settings is second_snapshot
                        assert first.execution.snapshot is first_snapshot
                        async with second.execution.operation("second.finish_stage") as stage:
                            assert stage.settings is second_snapshot
                        # Re-entering the original runtime keeps its already admitted root.
                        async with first.execution.operation("first.reenter") as reentered:
                            assert reentered.settings is first_snapshot
                        entered.set()
                        await release_cross.wait()
                    assert first.execution.snapshot is first_snapshot
                    left_second.set()
                    await release_first_tail.wait()

            async def close_second() -> None:
                closing_started.set()
                await second_stack.aclose()

            holding = asyncio.create_task(occupy_second())
            crossing = None
            closing = None
            try:
                await asyncio.wait_for(occupied.wait(), 2)
                crossing = asyncio.create_task(cross_instance())
                await asyncio.wait_for(queued.wait(), 2)
                assert not entered.is_set()
                assert second.execution.in_flight == 2
                assert first.configuration.snapshot is not first_snapshot
                closing = asyncio.create_task(close_second())
                await asyncio.wait_for(closing_started.wait(), 2)
                assert not closing.done()
                release_occupied.set()
                await asyncio.wait_for(entered.wait(), 2)
                assert not closing.done()
                release_cross.set()
                await asyncio.wait_for(left_second.wait(), 2)
                await asyncio.wait_for(closing, 2)
                assert not crossing.done()
                assert second.execution.in_flight == 0
                assert second.state == "stopped"
                async with open_instance(second_path, environment={}) as reopened:
                    assert reopened.descriptor.instance_id == second.descriptor.instance_id
                    assert all((await reopened.jobs.queues()).values())
                    async with reopened.execution.operation("second.reopened"):
                        assert not crossing.done()
                release_first_tail.set()
                await asyncio.wait_for(crossing, 2)
            finally:
                release_occupied.set()
                release_cross.set()
                release_first_tail.set()
                await asyncio.gather(
                    *(task for task in (holding, crossing, closing) if task is not None),
                    return_exceptions=True,
                )

    asyncio.run(scenario())


def test_cancelled_corrupt_export_waits_for_real_file_cleanup(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original_bytes = bytes(range(256)) * 12
            artifact = await runtime.artifacts.ingest(
                original_bytes, media_type="application/octet-stream"
            )
            blob = await runtime.artifacts.blob(artifact.ref)
            runtime.artifacts.content.path_for(blob).write_bytes(b"changed!" + original_bytes[8:])
            destination = tmp_path / "cancelled-export.bin"
            started, release = threading.Event(), threading.Event()
            copy_file = shutil.copyfileobj

            def delayed_copy(source: BinaryIO, output: BinaryIO, length: int) -> None:
                started.set()
                if not release.wait(5):
                    raise TimeoutError("copy worker was not released")
                copy_file(source, output, length)

            with patch("nous.storage.content.shutil.copyfileobj", delayed_copy):
                exporting = asyncio.create_task(runtime.artifacts.export(artifact.ref, destination))
                try:
                    assert await asyncio.to_thread(started.wait, 2)
                    exporting.cancel()
                    with pytest.raises(TimeoutError):
                        await asyncio.wait_for(asyncio.shield(exporting), timeout=0.05)
                finally:
                    release.set()
                    with pytest.raises(asyncio.CancelledError) as cancelled:
                        await exporting
                assert any("content.corrupt" in note for note in cancelled.value.__notes__)
                assert not destination.exists()

    asyncio.run(scenario())


@pytest.mark.parametrize("corruption", ["same_length", "truncated"])
def test_artifact_export_rejects_corrupt_bytes_without_overwriting_user_files(
    tmp_path: Path, corruption: str
) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original_bytes = bytes(range(256)) * 12
            artifact = await runtime.artifacts.ingest(
                original_bytes, media_type="application/octet-stream", display_name="original.bin"
            )
            valid = tmp_path / "valid.bin"
            assert await runtime.artifacts.export(artifact.ref, valid) == valid
            assert valid.read_bytes() == original_bytes
            blob = await runtime.artifacts.blob(artifact.ref)
            body = runtime.artifacts.content.path_for(blob)
            corrupted = (
                b"changed!" + original_bytes[8:]
                if corruption == "same_length"
                else original_bytes[:-1]
            )
            body.write_bytes(corrupted)
            destination = tmp_path / "must-not-succeed.bin"
            with pytest.raises(NousError) as failed:
                await runtime.artifacts.export(artifact.ref, destination)
            assert failed.value.code == "content.corrupt"
            assert not destination.exists()
            protected = tmp_path / "existing-user-file.bin"
            protected.write_bytes(b"keep this user's file")
            with pytest.raises(FileExistsError):
                await runtime.artifacts.export(artifact.ref, protected)
            assert protected.read_bytes() == b"keep this user's file"

    asyncio.run(scenario())
