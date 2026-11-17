"""FFmpeg subprocess ownership, stream inspection and exact frame/time selection."""

from __future__ import annotations

import asyncio
import json
import shutil
import tempfile
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.runtime.execution import ExecutionManager, acquire_owned, stop_process

from .configuration import MediaConfig


class AudioVisual:
    """Own FFmpeg children until exit, including cleanup during cancellation."""

    def __init__(self, settings: MediaConfig, execution: ExecutionManager) -> None:
        self.execution = execution
        self.ffmpeg = self._resolve(settings.ffmpeg_path, "ffmpeg")
        self.ffprobe = self._resolve(settings.ffprobe_path, "ffprobe")
        self._version: str | None = None

    @staticmethod
    def _resolve(configured: str | None, executable: str) -> str:
        target = configured or shutil.which(executable)
        if not target:
            raise NousError(
                "media.executable_missing",
                f"{executable} is required for audio/video",
                details={"executable": executable},
            )
        return target

    async def process(self, *args: str) -> bytes:
        try:
            process = await acquire_owned(
                asyncio.create_subprocess_exec(
                    *args,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                ),
                stop_process,
            )
        except OSError as exc:
            raise NousError(
                "media.executable_missing",
                "Cannot start media executable",
                details={"executable": args[0]},
            ) from exc
        communication = asyncio.create_task(process.communicate())
        try:
            stdout, stderr = await asyncio.shield(communication)
        except BaseException as primary:

            async def stop() -> None:
                if process.returncode is None:
                    try:
                        process.terminate()
                    except ProcessLookupError:
                        pass
                try:
                    await asyncio.wait_for(
                        asyncio.shield(communication),
                        timeout=float(
                            self.execution.snapshot.values.runtime.shutdown_grace_seconds
                        ),
                    )
                except TimeoutError:
                    if process.returncode is None:
                        try:
                            process.kill()
                        except ProcessLookupError:
                            pass
                    await communication

            cleanup = asyncio.create_task(stop())
            while not cleanup.done():
                try:
                    await asyncio.shield(cleanup)
                except asyncio.CancelledError:
                    continue
            try:
                cleanup.result()
            except BaseException as error:
                primary.add_note(f"Subprocess cleanup failed: {type(error).__name__}: {error}")
            raise primary
        if process.returncode:
            raise NousError(
                "media.conversion_failed",
                "Media executable failed",
                details={
                    "executable": args[0],
                    "stderr": stderr.decode("utf-8", errors="replace")[-4000:],
                },
            )
        return stdout

    async def version(self) -> str:
        if self._version is None:
            self._version = (await self.process(self.ffmpeg, "-version")).decode().splitlines()[0]
        return self._version

    async def inspect(self, path: Path) -> dict[str, JsonValue]:
        data = json.loads(
            await self.process(
                self.ffprobe,
                "-v",
                "error",
                "-show_format",
                "-show_streams",
                "-of",
                "json",
                str(path),
            )
        )
        streams = cast(list[dict[str, JsonValue]], data.get("streams", []))
        raw_duration = data.get("format", {}).get("duration")
        try:
            duration = float(raw_duration) if raw_duration is not None else None
        except TypeError, ValueError:
            duration = None
        video = any(s.get("codec_type") == "video" for s in streams)
        format_name = str(data.get("format", {}).get("format_name", "")).split(",")[0]
        mime = {
            "wav": "audio/wav",
            "mp3": "audio/mpeg",
            "flac": "audio/flac",
            "ogg": "video/ogg" if video else "audio/ogg",
            "matroska": "video/x-matroska" if video else "audio/x-matroska",
            "webm": "video/webm" if video else "audio/webm",
            "mov": "video/mp4" if video else "audio/mp4",
            "mpeg": "video/mpeg",
            "mpegts": "video/mp2t",
            "avi": "video/x-msvideo",
        }.get(format_name, "video/" + format_name if video else "audio/" + format_name)
        return {
            "kind": "video" if video else "audio",
            "media_type": mime,
            "duration_ms": round(duration * 1000) if duration is not None else None,
            "streams": cast(JsonValue, streams),
            "has_audio": any(s.get("codec_type") == "audio" for s in streams),
            "tool": "FFmpeg",
            "version": await self.version(),
        }

    async def frame_times(
        self, path: Path, start_ms: int = 0, end_ms: int | None = None, origin_ms: int = 0
    ) -> tuple[tuple[int, int], ...]:
        """Inspect only this bounded interval and retain actual source timestamps."""
        data = json.loads(
            await self.process(
                self.ffprobe,
                "-v",
                "error",
                "-select_streams",
                "v:0",
                *(
                    [
                        "-read_intervals",
                        f"{(start_ms + origin_ms) / 1000:.6f}%{(end_ms + origin_ms) / 1000:.6f}",
                    ]
                    if end_ms is not None
                    else []
                ),
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "json",
                str(path),
            )
        )
        times = [
            round(float(frame["best_effort_timestamp_time"]) * 1000) - origin_ms
            for frame in data.get("frames", [])
            if "best_effort_timestamp_time" in frame
        ]
        frames = [
            (index, time)
            for index, time in enumerate(
                time for time in times if time >= start_ms and (end_ms is None or time < end_ms)
            )
        ]
        if not frames:
            raise NousError("media.no_frames", "Video contains no decodable timed frames")
        return tuple(frames)

    @asynccontextmanager
    async def frames(
        self,
        path: Path,
        indices: tuple[int, ...],
        max_side: int,
        temporary: Path,
        start_ms: int = 0,
    ) -> AsyncGenerator[dict[int, Path]]:
        """Decode selected frames in one FFmpeg traversal; keep only bounded reads in memory."""
        if not indices:
            yield {}
            return
        await self.execution.blocking(temporary.mkdir, parents=True, exist_ok=True)
        directory = await acquire_owned(
            self.execution.blocking(tempfile.TemporaryDirectory, prefix="frames-", dir=temporary),
            lambda value: self.execution.control_blocking(value.cleanup),
        )
        try:
            output = Path(directory.name)
            selected = "+".join(f"eq(n\\,{index})" for index in indices)
            await self.process(
                self.ffmpeg,
                "-v",
                "error",
                "-ss",
                f"{start_ms / 1000:.6f}",
                "-i",
                str(path),
                "-vf",
                f"select={selected},scale=w='min({max_side},iw)':h='min({max_side},ih)':force_original_aspect_ratio=decrease",
                "-fps_mode",
                "passthrough",
                "-frames:v",
                str(len(indices)),
                str(output / "%06d.png"),
            )
            files = {
                index: output / f"{ordinal:06d}.png" for ordinal, index in enumerate(indices, 1)
            }
            if any(not file.is_file() for file in files.values()):
                raise NousError(
                    "media.missing_frame", "FFmpeg did not produce every selected frame"
                )
            yield files
        finally:
            await self.execution.blocking(directory.cleanup)

    async def audio(
        self, path: Path, start_ms: int, end_ms: int, media_type: str = "audio/wav"
    ) -> bytes:
        return await self.process(
            self.ffmpeg,
            "-v",
            "error",
            "-ss",
            f"{start_ms / 1000:.6f}",
            "-i",
            str(path),
            "-t",
            f"{(end_ms - start_ms) / 1000:.6f}",
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-f",
            {"audio/wav": "wav", "audio/mpeg": "mp3", "audio/flac": "flac"}[media_type],
            "pipe:1",
        )

    async def video(self, path: Path, start_ms: int, end_ms: int, *, without_audio: bool) -> bytes:
        """Encode an exact selected interval as an MP4, preserving the requested audio policy."""
        return await self.process(
            self.ffmpeg,
            "-v",
            "error",
            "-ss",
            f"{start_ms / 1000:.6f}",
            "-i",
            str(path),
            "-t",
            f"{(end_ms - start_ms) / 1000:.6f}",
            "-map",
            "0:v:0",
            *(["-an"] if without_audio else ["-map", "0:a:0?", "-c:a", "aac"]),
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-preset",
            "veryfast",
            "-crf",
            "20",
            "-movflags",
            "frag_keyframe+empty_moov",
            "-f",
            "mp4",
            "pipe:1",
        )
