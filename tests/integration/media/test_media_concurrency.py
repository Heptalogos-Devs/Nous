"""Concurrent media conversions release resources on failure and cancellation."""

import asyncio
import shutil
from pathlib import Path
from unittest.mock import patch

import pytest

from nous.content.contracts import OutputContent, SourceSelection
from nous.media.derivatives import MediaDerivatives
from nous.runtime.bootstrap import open_instance


def test_derivative_failure_and_cancellation_release_same_key(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import initialize_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("original")
            derivatives = MediaDerivatives(runtime.artifacts, runtime.results)
            output = {
                "value": OutputContent(
                    data=b"prepared", media_type="text/plain", display_name="prepared.txt"
                )
            }
            adopt = runtime.results.adopt
            attempts = 0

            async def fail_once(*args: object, **kwargs: object) -> object:
                nonlocal attempts
                attempts += 1
                if attempts == 1:
                    raise OSError("temporary failure")
                return await adopt(*args, **kwargs)  # type: ignore[arg-type]

            with patch.object(runtime.results, "adopt", side_effect=fail_once):
                with pytest.raises(OSError):
                    await derivatives.derive(
                        original, SourceSelection(), "test.derivative", output, {}
                    )
                result = await derivatives.derive(
                    original, SourceSelection(), "test.derivative", output, {}
                )
                same = await derivatives.derive(
                    original, SourceSelection(), "test.derivative", output, {}
                )
            assert result["value"].ref == same["value"].ref
            started, release = asyncio.Event(), asyncio.Event()

            async def delayed(*args: object, **kwargs: object) -> object:
                started.set()
                await release.wait()
                return await adopt(*args, **kwargs)  # type: ignore[arg-type]

            with patch.object(runtime.results, "adopt", side_effect=delayed):
                first = asyncio.create_task(
                    derivatives.derive(original, SourceSelection(), "test.cancel", output, {})
                )
                await started.wait()
                second = asyncio.create_task(
                    derivatives.derive(original, SourceSelection(), "test.cancel", output, {})
                )
                first.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await first
                release.set()
                recovered = await second
            assert await runtime.artifacts.read_bytes(recovered["value"].ref) == b"prepared"
            converting = asyncio.Event()

            async def interrupted_conversion() -> dict[str, OutputContent]:
                converting.set()
                await asyncio.Event().wait()
                return output

            async def retry_conversion() -> dict[str, OutputContent]:
                return output

            first = asyncio.create_task(
                derivatives.get_or_create(
                    original,
                    SourceSelection(),
                    "test.cancel_conversion",
                    {},
                    interrupted_conversion,
                )
            )
            await converting.wait()
            second = asyncio.create_task(
                derivatives.get_or_create(
                    original, SourceSelection(), "test.cancel_conversion", {}, retry_conversion
                )
            )
            first.cancel()
            with pytest.raises(asyncio.CancelledError):
                await first
            assert await runtime.artifacts.read_bytes((await second)["value"].ref) == b"prepared"

    asyncio.run(scenario())


def test_repeated_cancellation_waits_for_real_ffmpeg_exit(tmp_path: Path) -> None:
    from nous.media.audiovisual import AudioVisual
    from nous.runtime.bootstrap import initialize_instance

    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("Media processing requires FFmpeg and ffprobe")

    async def scenario() -> None:
        instance = tmp_path / "cancel-ffmpeg"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            media = AudioVisual(runtime.configuration.snapshot.values.media, runtime.execution)
            started = asyncio.Event()
            processes: list[asyncio.subprocess.Process] = []
            spawn = asyncio.create_subprocess_exec

            async def capture(*args: object, **kwargs: object) -> asyncio.subprocess.Process:
                process = await spawn(*args, **kwargs)  # type: ignore[arg-type]
                processes.append(process)
                started.set()
                return process

            async with runtime.execution.operation("media.cancel_check"):
                with patch("nous.media.audiovisual.asyncio.create_subprocess_exec", new=capture):
                    task = asyncio.create_task(
                        media.process(
                            media.ffmpeg,
                            "-v",
                            "error",
                            "-re",
                            "-f",
                            "lavfi",
                            "-i",
                            "sine=frequency=440",
                            "-t",
                            "20",
                            "-f",
                            "null",
                            "-",
                        )
                    )
                    await asyncio.wait_for(started.wait(), 10)
                    task.cancel()
                    task.cancel()
                    with pytest.raises(asyncio.CancelledError):
                        await task
            assert len(processes) == 1 and processes[0].returncode is not None

    asyncio.run(scenario())
