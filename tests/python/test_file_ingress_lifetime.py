"""External file ingress cannot strand an SDK host event loop."""

import asyncio
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

from nous.runtime.bootstrap import initialize_instance


@pytest.mark.skipif(os.name == "nt", reason="POSIX named pipe is required")
def test_sdk_slow_file_does_not_block_timeout_or_other_work(tmp_path: Path) -> None:
    instance, fifo = tmp_path / "instance", tmp_path / "slow.fifo"
    asyncio.run(initialize_instance(instance, environment={}))
    os.mkfifo(fifo)
    program = """import asyncio, sys
from pathlib import Path
from nous.runtime.bootstrap import open_instance
async def run():
    async with open_instance(Path(sys.argv[1]), environment={}, overrides={"execution.default_timeout_seconds": 1}) as runtime:
        async def other():
            await asyncio.sleep(0.1)
            source = await runtime.artifacts.ingest("independent work")
            assert await runtime.artifacts.read_text(source.ref) == "independent work"
            print("OTHER_WORK_COMPLETED", flush=True)
        other_task = asyncio.create_task(other())
        try:
            await runtime.artifacts.ingest(Path(sys.argv[2]))
        except TimeoutError:
            print("FILE_TIMED_OUT", flush=True)
        else:
            raise AssertionError("expected the file timeout")
        await other_task
        cancelled = asyncio.create_task(runtime.artifacts.ingest(Path(sys.argv[2])))
        await asyncio.sleep(0.2)
        cancelled.cancel()
        try:
            await cancelled
        except asyncio.CancelledError:
            print("FILE_CANCELLED", flush=True)
        else:
            raise AssertionError("expected cancellation")
    print("HOST_EXITED", flush=True)
asyncio.run(run())
"""
    try:
        result = subprocess.run(
            [sys.executable, "-c", program, str(instance), str(fifo)],
            text=True,
            capture_output=True,
            timeout=12,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        pytest.fail(f"SDK file ingress blocked the event loop or shutdown: {error}")
    assert result.returncode == 0, (result.stdout, result.stderr)
    assert "OTHER_WORK_COMPLETED" in result.stdout
    assert "FILE_TIMED_OUT" in result.stdout
    assert "FILE_CANCELLED" in result.stdout
    assert "HOST_EXITED" in result.stdout


@pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="Linux process inspection is required"
)
def test_reader_exits_if_its_sdk_host_is_killed(tmp_path: Path) -> None:
    instance, fifo, marker = tmp_path / "instance", tmp_path / "slow.fifo", tmp_path / "ready"
    child_marker = tmp_path / "reader-pid"
    asyncio.run(initialize_instance(instance, environment={}))
    os.mkfifo(fifo)
    program = """import asyncio, sys
from pathlib import Path
from nous.runtime.bootstrap import open_instance
async def run():
    async with open_instance(Path(sys.argv[1]), environment={}, overrides={"execution.default_timeout_seconds": None}) as runtime:
        original = asyncio.create_subprocess_exec
        async def tracked(*args, **kwargs):
            process = await original(*args, **kwargs)
            Path(sys.argv[4]).write_text(str(process.pid))
            return process
        asyncio.create_subprocess_exec = tracked
        Path(sys.argv[3]).write_text("ready")
        await runtime.artifacts.ingest(Path(sys.argv[2]))
asyncio.run(run())
"""
    host = subprocess.Popen(
        [sys.executable, "-c", program, str(instance), str(fifo), str(marker), str(child_marker)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    reader: int | None = None
    try:
        deadline = time.monotonic() + 20
        while not child_marker.exists():
            assert host.poll() is None and time.monotonic() < deadline
            time.sleep(0.05)
        reader = int(child_marker.read_text())
        assert b"NOUS_INGRESS_READER" in Path(f"/proc/{reader}/cmdline").read_bytes()
        assert marker.exists()
        host.kill()
        host.wait(timeout=10)
        deadline = time.monotonic() + 10
        while True:
            try:
                state = Path(f"/proc/{reader}/stat").read_text().split()[2]
            except FileNotFoundError:
                break
            if state in {"Z", "X"}:
                break
            assert time.monotonic() < deadline, "File reader survived its killed host"
            time.sleep(0.05)
    finally:
        if host.poll() is None:
            host.kill()
            host.wait(timeout=10)
        if reader is not None:
            try:
                command = Path(f"/proc/{reader}/cmdline").read_bytes()
                if b"NOUS_INGRESS_READER" in command:
                    os.kill(reader, signal.SIGKILL)
            except FileNotFoundError, ProcessLookupError:
                pass


def test_binary_file_stream_and_missing_path(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import open_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        payload = b"\x00\r\n\xff" * 400_000
        source = tmp_path / "binary.dat"
        source.write_bytes(payload)
        async with open_instance(instance, environment={}) as runtime:
            artifact = await runtime.artifacts.ingest(source)
            assert await runtime.artifacts.read_bytes(artifact.ref) == payload
            with pytest.raises(FileNotFoundError):
                await runtime.artifacts.ingest(tmp_path / "missing.dat")

    asyncio.run(scenario())


def test_cancel_during_reader_launch_reaps_late_process(tmp_path: Path) -> None:
    from unittest.mock import patch

    from nous.storage.file_input import file_chunks

    async def scenario() -> None:
        started, release = asyncio.Event(), asyncio.Event()
        processes: list[asyncio.subprocess.Process] = []
        original = asyncio.create_subprocess_exec

        async def delayed(*args: str, **kwargs: object) -> asyncio.subprocess.Process:
            started.set()
            await release.wait()
            process = await original(*args, **kwargs)  # type: ignore[arg-type]
            processes.append(process)
            return process

        stream = file_chunks(tmp_path / "file", 1024)
        with patch("asyncio.create_subprocess_exec", new=delayed):
            task = asyncio.create_task(anext(stream))
            await started.wait()
            task.cancel()
            release.set()
            with pytest.raises(asyncio.CancelledError):
                await task
        assert len(processes) == 1 and processes[0].returncode is not None
        await stream.aclose()

    asyncio.run(scenario())


def test_cancel_during_media_launch_reaps_late_process(tmp_path: Path) -> None:
    from unittest.mock import patch

    from nous.media.audiovisual import AudioVisual
    from nous.runtime.bootstrap import open_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        started, release = asyncio.Event(), asyncio.Event()
        processes: list[asyncio.subprocess.Process] = []
        original = asyncio.create_subprocess_exec

        async def delayed(*args: str, **kwargs: object) -> asyncio.subprocess.Process:
            started.set()
            await release.wait()
            process = await original(*args, **kwargs)  # type: ignore[arg-type]
            processes.append(process)
            return process

        async with open_instance(instance, environment={}) as runtime:
            settings = runtime.configuration.snapshot.values.media.model_copy(
                update={"ffmpeg_path": sys.executable, "ffprobe_path": sys.executable}
            )
            media = AudioVisual(settings, runtime.execution)
            with patch("asyncio.create_subprocess_exec", new=delayed):
                task = asyncio.create_task(
                    media.process(sys.executable, "-c", "import time; time.sleep(60)")
                )
                await asyncio.wait_for(started.wait(), timeout=10)
                task.cancel()
                release.set()
                with pytest.raises(asyncio.CancelledError):
                    await task
            assert len(processes) == 1 and processes[0].returncode is not None

    asyncio.run(scenario())


def test_file_readers_are_reused_and_closed_with_runtime(tmp_path: Path) -> None:
    from unittest.mock import patch

    from nous.runtime.bootstrap import open_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        processes: list[asyncio.subprocess.Process] = []
        original = asyncio.create_subprocess_exec

        async def tracked(*args: str, **kwargs: object) -> asyncio.subprocess.Process:
            process = await original(*args, **kwargs)  # type: ignore[arg-type]
            processes.append(process)
            return process

        async with open_instance(instance, environment={}) as runtime:
            with patch("asyncio.create_subprocess_exec", new=tracked):
                for index in range(4):
                    source = tmp_path / f"file-{index}"
                    payload = f"source {index}".encode()
                    source.write_bytes(payload)
                    artifact = await runtime.artifacts.ingest(source)
                    assert await runtime.artifacts.read_bytes(artifact.ref) == payload
                assert len(processes) == 1 and processes[0].returncode is None
        assert processes[0].returncode is not None

    asyncio.run(scenario())


@pytest.mark.skipif(os.name == "nt", reason="POSIX named pipe is required")
def test_bounded_reader_pool_cancels_only_its_blocked_worker(tmp_path: Path) -> None:
    from unittest.mock import patch

    from nous.runtime.bootstrap import open_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        first_fifo, second_fifo = tmp_path / "first.fifo", tmp_path / "second.fifo"
        os.mkfifo(first_fifo)
        os.mkfifo(second_fifo)
        regular = tmp_path / "regular"
        regular.write_bytes(b"independent source")
        processes: list[asyncio.subprocess.Process] = []
        ready = asyncio.Event()
        original = asyncio.create_subprocess_exec

        async def tracked(*args: str, **kwargs: object) -> asyncio.subprocess.Process:
            process = await original(*args, **kwargs)  # type: ignore[arg-type]
            processes.append(process)
            if len(processes) >= 2:
                ready.set()
            return process

        async with open_instance(
            instance, environment={}, overrides={"execution.max_blocking_workers": 2}
        ) as runtime:
            with patch("asyncio.create_subprocess_exec", new=tracked):
                first = asyncio.create_task(runtime.artifacts.ingest(first_fifo))
                second = asyncio.create_task(runtime.artifacts.ingest(second_fifo))
                await asyncio.wait_for(ready.wait(), timeout=15)
                queued = asyncio.create_task(runtime.artifacts.ingest(regular))
                await asyncio.sleep(0.2)
                assert not queued.done()
                first.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await first
                artifact = await asyncio.wait_for(queued, timeout=10)
                assert await runtime.artifacts.read_bytes(artifact.ref) == b"independent source"
                assert not second.done()
                second.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await second
        assert all(process.returncode is not None for process in processes)

    asyncio.run(scenario())


def test_reader_pool_finishes_reaping_despite_repeated_close_cancellation(tmp_path: Path) -> None:
    from unittest.mock import patch

    from nous.storage import file_input

    async def scenario() -> None:
        source = tmp_path / "file"
        source.write_bytes(b"contents")
        pool = file_input.FileReaderPool(1)
        assert b"".join([block async for block in pool.chunks(source, 1024)]) == b"contents"
        process = pool._idle[0]  # pyright: ignore[reportPrivateUsage]
        started, release = asyncio.Event(), asyncio.Event()
        original = file_input.stop_process

        async def delayed_stop(worker: asyncio.subprocess.Process) -> None:
            started.set()
            await release.wait()
            await original(worker)

        try:
            with patch("nous.storage.file_input.stop_process", new=delayed_stop):
                task = asyncio.create_task(pool.aclose())
                await asyncio.wait_for(started.wait(), timeout=10)
                task.cancel()
                task.cancel()
                release.set()
                with pytest.raises(asyncio.CancelledError):
                    await task
            assert process.returncode is not None
            await pool.aclose()
        finally:
            if process.returncode is None:
                await original(process)

    asyncio.run(scenario())


def test_runtime_reaps_file_readers_if_shutdown_is_cancelled(tmp_path: Path) -> None:
    from collections.abc import Callable
    from unittest.mock import patch

    from nous.runtime.bootstrap import open_instance

    async def scenario() -> None:
        instance, source = tmp_path / "instance", tmp_path / "file"
        await initialize_instance(instance, environment={})
        source.write_bytes(b"contents")
        ready, drain_started, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
        processes: list[asyncio.subprocess.Process] = []
        stop_patches: list[Callable[[], object]] = []
        original = asyncio.create_subprocess_exec

        async def tracked(*args: str, **kwargs: object) -> asyncio.subprocess.Process:
            process = await original(*args, **kwargs)  # type: ignore[arg-type]
            processes.append(process)
            return process

        async def owner() -> None:
            async with open_instance(instance, environment={}) as runtime:
                real_drain = runtime.execution.drain

                async def delayed_drain(grace_seconds: float = 30) -> None:
                    drain_started.set()
                    await release.wait()
                    await real_drain(grace_seconds)

                patcher = patch.object(runtime.execution, "drain", new=delayed_drain)
                patcher.start()
                stop_patches.append(patcher.stop)
                await runtime.artifacts.ingest(source)
                ready.set()
                await asyncio.Event().wait()

        task = None
        try:
            with patch("asyncio.create_subprocess_exec", new=tracked):
                task = asyncio.create_task(owner())
                await asyncio.wait_for(ready.wait(), timeout=20)
                task.cancel()
                await asyncio.wait_for(drain_started.wait(), timeout=10)
                task.cancel()
                release.set()
                with pytest.raises(asyncio.CancelledError):
                    await asyncio.wait_for(task, timeout=15)
            assert len(processes) == 1 and processes[0].returncode is not None
        finally:
            release.set()
            if task is not None and not task.done():
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
            for stop in stop_patches:
                stop()
            for process in processes:
                if process.returncode is None:
                    process.kill()
                    await process.wait()

    asyncio.run(scenario())


@pytest.mark.skipif(os.name == "nt", reason="POSIX named pipe and symlink are required")
def test_reused_file_reader_resolves_caller_cwd_without_collapsing_symlinks(tmp_path: Path) -> None:
    from nous.storage.file_input import FileReaderPool

    async def read(pool: FileReaderPool, path: Path) -> bytes:
        return b"".join([block async for block in pool.chunks(path, 1024)])

    async def scenario() -> None:
        original = Path.cwd()
        first, second = tmp_path / "first", tmp_path / "second"
        first.mkdir()
        (second / "subdirectory").mkdir(parents=True)
        (first / "entry").write_bytes(b"first")
        (second / "entry").write_bytes(b"second")
        (first / "link").symlink_to(second / "subdirectory", target_is_directory=True)
        os.mkfifo(first / "slow.fifo")
        pool = FileReaderPool(1)
        blocked = None
        queued = None
        try:
            os.chdir(first)
            assert await read(pool, Path("entry")) == b"first"
            os.chdir(second)
            assert await read(pool, Path("entry")) == b"second"
            os.chdir(first)
            assert await read(pool, Path("link/../entry")) == b"second"
            blocked = asyncio.create_task(read(pool, Path("slow.fifo")))
            await asyncio.sleep(0.2)
            assert not blocked.done()
            queued = asyncio.create_task(read(pool, Path("entry")))
            await asyncio.sleep(0.1)
            assert not queued.done()
            os.chdir(second)
            blocked.cancel()
            with pytest.raises(asyncio.CancelledError):
                await blocked
            assert await asyncio.wait_for(queued, timeout=10) == b"first"
        finally:
            os.chdir(original)
            for task in (blocked, queued):
                if task is not None and not task.done():
                    task.cancel()
                    await asyncio.gather(task, return_exceptions=True)
            await pool.aclose()

    asyncio.run(scenario())


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="Linux procfs is required")
def test_file_reader_preserves_caller_process_and_thread_identity(tmp_path: Path) -> None:
    import threading

    from nous.storage.file_input import FileReaderPool

    link = tmp_path / "status-link"
    link.symlink_to("/proc/self/status")
    results: list[tuple[int, bytes, bytes, bytes]] = []

    async def scenario() -> None:
        pool = FileReaderPool(1)

        async def read(path: Path) -> bytes:
            return b"".join([block async for block in pool.chunks(path, 4096)])

        try:
            direct = await read(Path("/proc/self/status"))
            through_link = await read(link)
            thread = await read(Path("/proc/thread-self/status"))
            with pytest.raises(FileNotFoundError):
                await read(tmp_path / "missing/../../proc/self/status")
            results.append((threading.get_native_id(), direct, through_link, thread))
        finally:
            await pool.aclose()

    worker = threading.Thread(target=lambda: asyncio.run(scenario()))
    worker.start()
    worker.join(timeout=15)
    assert not worker.is_alive()
    assert len(results) == 1
    thread_id, direct, through_link, thread = results[0]
    assert f"Pid:\t{os.getpid()}\n".encode() in direct
    assert f"Pid:\t{os.getpid()}\n".encode() in through_link
    assert f"Tgid:\t{os.getpid()}\n".encode() in thread
    assert f"Pid:\t{thread_id}\n".encode() in thread
