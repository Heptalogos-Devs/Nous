"""FFmpeg subprocess ownership, stream inspection and exact frame/time selection."""

from __future__ import annotations

import asyncio
import json
import shutil
from pathlib import Path
from typing import cast

from pydantic import JsonValue

from nous.common.errors import NousError

from .configuration import MediaConfig


class AudioVisual:
    """Own FFmpeg children until exit, including cleanup during cancellation."""

    def __init__(self, settings: MediaConfig) -> None:
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
            process = await asyncio.create_subprocess_exec(
                *args,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except OSError as exc:
            raise NousError(
                "media.executable_missing",
                "Cannot start media executable",
                details={"executable": args[0]},
            ) from exc
        try:
            stdout, stderr = await process.communicate()
        except BaseException:
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.communicate(), timeout=5)
                except TimeoutError:
                    process.kill()
                    await process.communicate()
            raise
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
        duration = float(data.get("format", {}).get("duration", 0))
        return {
            "kind": "video" if any(s.get("codec_type") == "video" for s in streams) else "audio",
            "duration_ms": round(duration * 1000),
            "streams": cast(JsonValue, streams),
            "has_audio": any(s.get("codec_type") == "audio" for s in streams),
            "tool": "FFmpeg",
            "version": await self.version(),
        }

    async def frame_times(self, path: Path) -> tuple[tuple[int, int], ...]:
        """Return decoded frame indices and actual timestamps relative to the first video frame."""
        data = json.loads(
            await self.process(
                self.ffprobe,
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "json",
                str(path),
            )
        )
        frames = [
            (index, round(float(frame["best_effort_timestamp_time"]) * 1000))
            for index, frame in enumerate(data.get("frames", []))
            if "best_effort_timestamp_time" in frame
        ]
        if not frames:
            raise NousError("media.no_frames", "Video contains no decodable timed frames")
        origin = frames[0][1]
        return tuple((index, time - origin) for index, time in frames)

    async def frame(self, path: Path, index: int, max_side: int) -> bytes:
        # Select by the decoded frame index from ffprobe; timestamps are never guessed from FPS.
        return await self.process(
            self.ffmpeg,
            "-v",
            "error",
            "-i",
            str(path),
            "-vf",
            f"select=eq(n\\,{index}),scale=w='min({max_side},iw)':h='min({max_side},ih)':force_original_aspect_ratio=decrease",
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "pipe:1",
        )

    async def audio(self, path: Path, start_ms: int, end_ms: int) -> bytes:
        return await self.process(
            self.ffmpeg,
            "-v",
            "error",
            "-i",
            str(path),
            "-ss",
            f"{start_ms / 1000:.6f}",
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
            "wav",
            "pipe:1",
        )
