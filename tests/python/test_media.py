"""Real content lifetime and Pillow/FFmpeg integration boundaries."""

import asyncio
import io
import math
import os
import struct
import sys
import time
import wave
from pathlib import Path

import pytest
from nous_wave.contracts import CallContext, PixelRect, TimeSpan
from PIL import Image

from nous.platform.content import ContentStore
from nous.platform.execution import DurableExecution, JobSpec
from nous.platform.media import MediaService, run_process
from nous.platform.media import service as media_service
from nous.platform.persistence import Authority


def test_shared_content_references_and_rollback_leases_control_purge(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        store = ContentStore(authority, tmp_path / "content", max_bytes=8)
        await authority.initialize()
        await store.initialize()
        ctx = CallContext(credential_source_ids=None, caller_id="owner", namespace_id="namespace")
        first = await store.publish_bytes(b"original", "application/octet-stream")
        second = await store.publish_bytes(b"original", "application/octet-stream")
        assert first.content_hash == second.content_hash
        async with authority.write(
            ctx, "attach", operation_kind="material.attach", payload={}
        ) as tx:
            await store.attach_in(tx, first, "material:a")
            await store.attach_in(tx, second, "material:b")
            tx.result({"attached": True})
        await store.release(first)
        await store.release(second)
        other = CallContext(
            credential_source_ids=None, caller_id="owner", namespace_id="other-namespace"
        )
        third = await store.publish_bytes(b"original", "application/octet-stream")
        async with authority.write(
            other, "attach", operation_kind="material.attach", payload={}
        ) as tx:
            await store.attach_in(tx, third, "material:a")
            tx.result(None)
        await store.release(third)
        async with authority.write(
            ctx, "detach-a", operation_kind="material.detach", payload={}
        ) as tx:
            await store.detach_in(tx, "material:a")
            tx.result(None)
        assert await store.purge(ctx, (first.content_hash,)) == ()
        async with store.protect(first) as original:
            async with authority.write(
                ctx, "detach-b", operation_kind="material.detach", payload={}
            ) as tx:
                await store.detach_in(tx, "material:b")
                tx.result(None)
            assert await store.purge(ctx, (first.content_hash,)) == ()
            assert original.read_bytes() == b"original"
        assert await store.purge(ctx, (first.content_hash,)) == ()
        async with authority.write(
            other, "detach", operation_kind="material.detach", payload={}
        ) as tx:
            await store.detach_in(tx, "material:a")
            tx.result(None)
        assert await store.purge(ctx, (first.content_hash,)) == (first.content_hash,)
        with pytest.raises(ValueError, match="size limit"):
            await store.publish_bytes(b"too-large", "application/octet-stream")
        assert list(store.staging.iterdir()) == []
        with pytest.raises(ValueError):
            store.path_for("../outside")
        await authority.close()

    asyncio.run(scenario())


def test_image_crop_preserves_original_and_exact_coordinates(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        store = ContentStore(authority, tmp_path / "content")
        await authority.initialize()
        media = MediaService(store)
        image = Image.new("RGB", (12, 8), "red")
        image.putpixel((5, 3), (0, 255, 0))
        buffer = io.BytesIO()
        image.save(buffer, format="PNG")
        source = await store.publish_bytes(buffer.getvalue(), "image/png")
        info = await media.probe(source)
        assert (info.kind, info.width, info.height) == ("image", 12, 8)
        cropped = await media.crop_image(source, PixelRect(x=5, y=3, width=2, height=2))
        with Image.open(store.path_for(cropped.lease.content_hash)) as result:
            assert result.size == (2, 2)
            assert result.getpixel((0, 0)) == (0, 255, 0)
        assert store.path_for(source.content_hash).read_bytes() == buffer.getvalue()
        with pytest.raises(ValueError, match="outside"):
            await media.crop_image(source, PixelRect(x=11, y=7, width=2, height=2))
        await store.release(cropped.lease)
        await store.release(source)
        await authority.close()

    asyncio.run(scenario())


def test_audio_clip_and_video_frame_preparation_use_real_codecs(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        store = ContentStore(authority, tmp_path / "content")
        await authority.initialize()
        media = MediaService(store)
        audio_path = tmp_path / "audio.wav"
        with wave.open(str(audio_path), "wb") as output:
            output.setnchannels(1)
            output.setsampwidth(2)
            output.setframerate(8000)
            output.writeframes(
                b"".join(
                    struct.pack("<h", round(12000 * math.sin(index * 2 * math.pi * 440 / 8000)))
                    for index in range(8000)
                )
            )
        audio = await store.publish_file(audio_path, "audio/wav")
        info = await media.probe(audio)
        assert info.kind == "audio" and info.duration_ms == 1000
        clip = await media.extract_clip(audio, TimeSpan(start_ms=200, end_ms=700))
        assert (await media.probe(clip.lease)).duration_ms == 500
        with pytest.raises(ValueError, match="outside"):
            await media.extract_clip(audio, TimeSpan(start_ms=0, end_ms=1100))
        video_path = tmp_path / "video.mp4"
        await run_process(
            (
                "ffmpeg",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=64x48:r=4:d=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=8000:duration=1",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                str(video_path),
            )
        )
        video = await store.publish_file(video_path, "video/mp4")
        cropped = await media.crop_video(
            video, PixelRect(x=1, y=1, width=17, height=13), TimeSpan(start_ms=200, end_ms=700)
        )
        cropped_info = await media.probe(cropped.lease)
        assert (cropped_info.width, cropped_info.height, cropped_info.channels) == (17, 13, 1)
        assert cropped.time_span == TimeSpan(start_ms=200, end_ms=700)
        frames = await media.sample_frames(
            video, (0, 500), rect=PixelRect(x=0, y=0, width=16, height=12)
        )
        assert [frame.at_ms for frame in frames] == [0, 500]
        assert frames[0].source_hash == video.content_hash
        for frame in frames:
            info = await media.probe(frame.lease)
            assert (info.width, info.height) == (16, 12)
            await store.release(frame.lease)
        await store.release(clip.lease)
        await store.release(audio)
        await store.release(video)
        await store.release(cropped.lease)
        await authority.close()

    asyncio.run(scenario())


def test_process_cancellation_waits_for_actual_process_exit(tmp_path):
    async def scenario():
        pid_path = tmp_path / "pid"
        script = "import os,signal,sys,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); open(sys.argv[1],'w').write(str(os.getpid())); time.sleep(60)"
        task = asyncio.create_task(run_process((sys.executable, "-c", script, str(pid_path))))

        def wait_pid():
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if pid_path.exists() and pid_path.read_text():
                    return int(pid_path.read_text())
                time.sleep(0.01)
            raise TimeoutError("Child did not start")

        pid = await asyncio.to_thread(wait_pid)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)

    asyncio.run(scenario())


def test_successful_early_transcode_is_rejected_before_content_publication(tmp_path, monkeypatch):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        store = ContentStore(authority, tmp_path / "content")
        await authority.initialize()
        media = MediaService(store)
        path = tmp_path / "source.wav"
        with wave.open(str(path), "wb") as original:
            original.setnchannels(1)
            original.setsampwidth(2)
            original.setframerate(8000)
            original.writeframes(bytes(16000))
        source = await store.publish_file(path, "audio/wav")
        measured = []

        async def early_success(arguments):
            if arguments[0] != "ffmpeg":
                return await run_process(arguments)
            # Fault injection retains the real encoder and successful process exit,
            # but makes it end early, as a size cap can. The file stays below budget.
            outcome = await run_process((*arguments[:-1], "-t", "0.1", arguments[-1]))
            measured.append((await asyncio.to_thread(Path(arguments[-1]).stat)).st_size)
            return outcome

        monkeypatch.setattr(media_service, "run_process", early_success)
        with pytest.raises(ValueError, match="full requested time interval"):
            await media.extract_clip(source, TimeSpan(start_ms=0, end_ms=800))
        assert 0 < measured[0] < media.policy.max_derivative_bytes
        assert list(store.staging.iterdir()) == []
        assert store.path_for(source.content_hash).read_bytes() == path.read_bytes()
        await store.release(source)
        await authority.close()

    asyncio.run(scenario())


def test_persistent_cancellation_reaches_preemptible_owned_subprocess(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        execution = DurableExecution(authority)
        pid_path = tmp_path / "owned-pid"
        script = "import os,sys,time; open(sys.argv[1],'w').write(str(os.getpid())); time.sleep(60)"

        async def prepare(reference):
            await run_process((sys.executable, "-c", script, str(pid_path)))
            return {"reference": reference}

        execution.register("preemptible-media", prepare, preemptible=True)
        await execution.start()
        ctx = CallContext(credential_source_ids=None, caller_id="owner", namespace_id="namespace")
        try:
            async with authority.write(ctx, "prepare", operation_kind="prepare", payload={}) as tx:
                job = await tx.enqueue(
                    JobSpec("preemptible-media", "default", args=("original-ref",))
                )
                tx.result({"job": job.workflow_id})

            def wait_pid():
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    if pid_path.exists() and pid_path.read_text():
                        return int(pid_path.read_text())
                    time.sleep(0.01)
                raise TimeoutError("Durable preparation did not start")

            pid = await asyncio.to_thread(wait_pid)
            await execution.cancel(ctx, job.workflow_id)

            def wait_stopped():
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    try:
                        os.kill(pid, 0)
                    except ProcessLookupError:
                        return
                    time.sleep(0.01)
                raise TimeoutError("Cancelled workflow retained its media subprocess")

            await asyncio.to_thread(wait_stopped)
            assert (await execution.status(ctx, job.workflow_id)).status == "CANCELLED"
        finally:
            await execution.close()
            await authority.close()

    asyncio.run(scenario())
