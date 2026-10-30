"""Pillow and FFmpeg media preparation over retained immutable originals."""

from __future__ import annotations

import asyncio
import math
import tempfile
from collections.abc import Awaitable, Callable
from copy import copy
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path

import anyio
from nous_sdk.contracts.media import MediaInfo, PixelRect, TimeSpan
from nous_sdk.contracts.media_policy import MediaPolicy
from PIL import Image, ImageOps
from pydantic import BaseModel, ConfigDict, Field

from nous.platform.content import ContentLease, ContentObject, ContentStore

from .process import run_process


class _Stream(BaseModel):
    model_config = ConfigDict(extra="ignore")
    codec_type: str | None = None
    width: int | None = None
    height: int | None = None
    duration: str | None = None
    start_time: str | None = None
    nb_frames: str | None = None
    avg_frame_rate: str | None = None
    sample_rate: str | None = None
    channels: int | None = None
    disposition: dict[str, int] = Field(default_factory=dict)


class _Format(BaseModel):
    model_config = ConfigDict(extra="ignore")
    duration: str | None = None
    start_time: str | None = None


class _Probe(BaseModel):
    model_config = ConfigDict(extra="ignore")
    streams: list[_Stream]
    format: _Format = Field(default_factory=_Format)


@dataclass(frozen=True)
class DerivedContent:
    lease: ContentLease
    source_hash: str
    transformation: str
    pixel_rect: PixelRect | None = None
    time_span: TimeSpan | None = None
    at_ms: int | None = None


class MediaService:
    def __init__(
        self,
        content: ContentStore,
        *,
        policy: MediaPolicy | None = None,
        policy_resolver: Callable[[], Awaitable[MediaPolicy]] | None = None,
    ):
        self.content = content
        self.policy = policy or MediaPolicy()
        self._policy_resolver = policy_resolver
        self._processes = anyio.CapacityLimiter(self.policy.process_concurrency)

    async def freeze(self) -> MediaService:
        """Freeze limits for this operation while sharing the instance process gate."""
        policy = await self._policy_resolver() if self._policy_resolver else self.policy
        self._processes.total_tokens = policy.process_concurrency
        operation = copy(self)
        operation.policy = policy.model_copy(deep=True)
        operation._policy_resolver = None
        return operation

    def _check_image(self, image: Image.Image) -> None:
        if image.width * image.height > self.policy.max_pixels:
            raise ValueError("Decoded image exceeds the instance pixel limit")
        if getattr(image, "n_frames", 1) > self.policy.max_frames:
            raise ValueError("Image exceeds the instance animation frame limit")

    def _image_info(self, path: Path, media_type: str) -> MediaInfo:
        with Image.open(path) as image:
            self._check_image(image)
            frames = int(getattr(image, "n_frames", 1))
            duration = 0
            for index in range(frames):
                image.seek(index)
                frame_duration = image.info.get("duration", 0)
                if isinstance(frame_duration, (int, float)):
                    duration += int(frame_duration)
            image.seek(0)
            oriented = ImageOps.exif_transpose(image)
            return MediaInfo(
                kind="image",
                media_type=media_type,
                width=oriented.width,
                height=oriented.height,
                frames=frames,
                duration_ms=duration if frames > 1 else None,
            )

    async def probe(self, source: ContentObject) -> MediaInfo:
        async with self.content.protect(source) as path:
            if source.media_type.startswith("image/"):
                try:
                    return await anyio.to_thread.run_sync(self._image_info, path, source.media_type)
                except OSError as error:
                    raise ValueError("The supplied image cannot be decoded") from error
            if not source.media_type.startswith(("video/", "audio/")):
                kind = (
                    "text"
                    if source.media_type.startswith("text/")
                    else "document"
                    if source.media_type == "application/pdf"
                    else "binary"
                )
                return MediaInfo(kind=kind, media_type=source.media_type)
            probe = await self._probe_file(path)
            video = next(
                (
                    stream
                    for stream in probe.streams
                    if stream.codec_type == "video" and stream.disposition.get("attached_pic") != 1
                ),
                None,
            )
            audio = next((stream for stream in probe.streams if stream.codec_type == "audio"), None)
            duration_ms = self._duration_ms(probe)
            if video is None and audio is None:
                raise ValueError("Media has no playable audio or video stream")
            return MediaInfo(
                kind="video" if video else "audio",
                media_type=source.media_type,
                width=video.width if video else None,
                height=video.height if video else None,
                frames=int(video.nb_frames)
                if video and video.nb_frames and video.nb_frames.isdigit()
                else None,
                duration_ms=duration_ms,
                sample_rate=int(audio.sample_rate) if audio and audio.sample_rate else None,
                channels=audio.channels if audio else None,
            )

    async def _probe_file(self, path: Path) -> _Probe:
        async with self._processes, asyncio.timeout(self.policy.probe_timeout_seconds):
            output = await run_process(
                (
                    self.policy.ffprobe,
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-show_streams",
                    "-show_format",
                    "-of",
                    "json",
                    str(path),
                )
            )
        return _Probe.model_validate_json(output.stdout)

    @staticmethod
    def _duration_ms(probe: _Probe) -> int | None:
        durations: list[float] = []
        for value in [probe.format.duration, *(stream.duration for stream in probe.streams)]:
            if value is not None:
                try:
                    seconds = float(value)
                except ValueError:
                    continue
                if math.isfinite(seconds) and seconds >= 0:
                    durations.append(seconds)
        return round(max(durations) * 1000) if durations else None

    @staticmethod
    def _milliseconds(value: str | None) -> int | None:
        try:
            seconds = float(value) if value is not None else None
        except ValueError:
            return None
        return round(seconds * 1000) if seconds is not None and math.isfinite(seconds) else None

    async def _check_coverage(
        self,
        output: Path,
        source: _Probe,
        expected_ms: int | None,
        span: TimeSpan | None,
        *,
        audio: bool,
    ) -> None:
        probe = await self._probe_file(output)
        actual_ms = self._duration_ms(probe)
        if expected_ms is None or actual_ms is None:
            raise ValueError("Cannot verify the prepared media time coverage")
        selected_start = span.start_ms if span is not None else 0
        selected_end = span.end_ms if span is not None else expected_ms
        origin = self._milliseconds(source.format.start_time) or 0
        sources = [
            stream
            for stream in source.streams
            if stream.codec_type in ({"audio"} if audio else {"audio", "video"})
            and stream.disposition.get("attached_pic") != 1
        ]
        if audio:
            sources = sources[:1]
        else:
            # The transcoder selects the first video and preserves all audio tracks.
            first_video = next((stream for stream in sources if stream.codec_type == "video"), None)
            sources = ([first_video] if first_video else []) + [
                stream for stream in sources if stream.codec_type == "audio"
            ]
        counters: dict[str, int] = {}
        tolerance_ms = 2
        for original in sources:
            kind = original.codec_type or ""
            index = counters.get(kind, 0)
            counters[kind] = index + 1
            outputs = [stream for stream in probe.streams if stream.codec_type == kind]
            if len(outputs) <= index:
                raise ValueError("Prepared media is missing a selected source stream")
            prepared = outputs[index]
            tolerance = 2
            if kind == "video" and prepared.avg_frame_rate is not None:
                try:
                    rate = Fraction(prepared.avg_frame_rate)
                except ValueError, ZeroDivisionError:
                    rate = Fraction(0)
                if rate > 0:
                    tolerance += math.ceil(1000 / rate)
            tolerance_ms = max(tolerance_ms, tolerance)
            source_duration = self._milliseconds(original.duration)
            required = expected_ms
            if source_duration is not None:
                original_start = self._milliseconds(original.start_time)
                start = (original_start if original_start is not None else origin) - origin
                required = max(
                    0, min(selected_end, start + source_duration) - max(selected_start, start)
                )
            covered = self._milliseconds(prepared.duration)
            if covered is None and len(probe.streams) == 1:
                covered = actual_ms
            if covered is None or covered + tolerance < required:
                raise ValueError("Prepared media does not cover the full requested time interval")
        if actual_ms + tolerance_ms < expected_ms:
            raise ValueError("Prepared media does not cover the full requested time interval")

    @staticmethod
    def _validate_rect(rect: PixelRect, width: int | None, height: int | None) -> None:
        if (
            width is None
            or height is None
            or rect.x + rect.width > width
            or rect.y + rect.height > height
        ):
            raise ValueError("Pixel rectangle is outside the decoded source image")

    def _crop_image(self, path: Path, destination: Path, rect: PixelRect) -> None:
        with Image.open(path) as image:
            self._check_image(image)
            if getattr(image, "n_frames", 1) != 1:
                raise ValueError("Animated images require frame sampling before image cropping")
            oriented = ImageOps.exif_transpose(image)
            self._validate_rect(rect, oriented.width, oriented.height)
            cropped = oriented.crop((rect.x, rect.y, rect.x + rect.width, rect.y + rect.height))
            cropped.save(destination, format="PNG")

    async def _publish_derivative(self, path: Path, media_type: str) -> ContentLease:
        if (await anyio.to_thread.run_sync(path.stat)).st_size > self.policy.max_derivative_bytes:
            raise ValueError("Prepared media exceeds the derivative size limit")
        return await self.content.publish_file(path, media_type)

    async def crop_image(self, source: ContentObject, rect: PixelRect) -> DerivedContent:
        info = await self.probe(source)
        if info.frames is not None and info.frames > 1:
            return await self.animate(source, rect=rect)
        async with self.content.protect(source) as path:
            with tempfile.TemporaryDirectory(
                prefix="nous-media-", dir=self.content.staging
            ) as temporary:
                output = Path(temporary) / "crop.png"
                await anyio.to_thread.run_sync(self._crop_image, path, output, rect)
                lease = await self._publish_derivative(output, "image/png")
                return DerivedContent(lease, source.content_hash, "image_crop", pixel_rect=rect)

    def _base(self) -> tuple[str, ...]:
        return (
            self.policy.ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-protocol_whitelist",
            "file,pipe",
            "-threads",
            "2",
            "-max_alloc",
            "268435456",
        )

    async def extract_clip(
        self, source: ContentObject, span: TimeSpan, *, audio_only: bool = False
    ) -> DerivedContent:
        info = await self.probe(source)
        if (
            info.kind not in ("audio", "video", "image")
            or info.duration_ms is None
            or span.end_ms > info.duration_ms
        ):
            raise ValueError("Time span is outside the source media duration")
        if info.kind == "image":
            if audio_only:
                raise ValueError("Animated images have no audio track")
            return await self.animate(source, span=span)
        return await self._transcode(source, info, span, None, audio_only)

    async def animate(
        self,
        source: ContentObject,
        *,
        rect: PixelRect | None = None,
        span: TimeSpan | None = None,
    ) -> DerivedContent:
        """Prepare one actual animation cycle; retain its temporal and spatial selection."""
        info = await self.probe(source)
        if info.kind != "image" or not info.frames or info.frames < 2 or not info.duration_ms:
            raise ValueError("Animation preparation requires timed, multiple image frames")
        if rect is not None:
            self._validate_rect(rect, info.width, info.height)
        if span is not None and span.end_ms > info.duration_ms:
            raise ValueError("Time span is outside the source animation duration")
        from .animation import animation_timeline

        selected = span or TimeSpan(start_ms=0, end_ms=info.duration_ms)
        duration = selected.end_ms - selected.start_ms
        async with self.content.protect(source) as path:
            with tempfile.TemporaryDirectory(
                prefix="nous-animation-", dir=self.content.staging
            ) as temporary:
                directory = Path(temporary)
                async with self._processes:
                    manifest = await anyio.to_thread.run_sync(
                        animation_timeline, path, directory, selected, rect
                    )
                output = directory / "animation.mp4"
                visual_filter = "null" if rect else "pad=ceil(iw/2)*2:ceil(ih/2)*2"
                pixel_format = (
                    "yuv444p" if rect and (rect.width % 2 or rect.height % 2) else "yuv420p"
                )
                arguments = self._base() + (
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    str(manifest),
                    "-t",
                    str(duration / 1000),
                    "-an",
                    "-vf",
                    visual_filter,
                    "-fps_mode",
                    "vfr",
                    "-enc_time_base",
                    "1:1000",
                    "-c:v",
                    "libx264",
                    "-bf",
                    "0",
                    "-pix_fmt",
                    pixel_format,
                    "-video_track_timescale",
                    "1000",
                    "-movflags",
                    "+faststart",
                    "-fs",
                    str(self.policy.max_derivative_bytes + 1),
                    str(output),
                )
                async with self._processes, asyncio.timeout(self.policy.transform_timeout_seconds):
                    await run_process(arguments)
                await self._check_coverage(
                    output, await self._probe_file(path), duration, selected, audio=False
                )
                if self._duration_ms(await self._probe_file(output)) != duration:
                    raise ValueError("Prepared animation changed the selected interval duration")
                lease = await self._publish_derivative(output, "video/mp4")
                return DerivedContent(
                    lease,
                    source.content_hash,
                    "video_crop" if rect else "video_clip",
                    pixel_rect=rect,
                    time_span=selected,
                )

    async def crop_video(
        self, source: ContentObject, rect: PixelRect, span: TimeSpan | None = None
    ) -> DerivedContent:
        info = await self.probe(source)
        if info.kind != "video":
            raise ValueError("Video cropping requires a video source")
        self._validate_rect(rect, info.width, info.height)
        if span is not None and (info.duration_ms is None or span.end_ms > info.duration_ms):
            raise ValueError("Time span is outside the source media duration")
        return await self._transcode(source, info, span, rect, False)

    async def _transcode(
        self,
        source: ContentObject,
        info: MediaInfo,
        span: TimeSpan | None,
        rect: PixelRect | None,
        audio_only: bool,
    ) -> DerivedContent:
        async with self.content.protect(source) as path:
            with tempfile.TemporaryDirectory(
                prefix="nous-media-", dir=self.content.staging
            ) as temporary:
                audio = audio_only or info.kind == "audio"
                output = Path(temporary) / ("clip.wav" if audio else "clip.mp4")
                arguments = self._base()
                if span is not None:
                    arguments += ("-ss", str(span.start_ms / 1000))
                arguments += ("-i", str(path))
                if span is not None:
                    arguments += ("-t", str((span.end_ms - span.start_ms) / 1000))
                if audio:
                    arguments += ("-map", "0:a:0", "-vn", "-c:a", "pcm_s16le", "-f", "wav")
                else:
                    visual_filter = (
                        f"crop={rect.width}:{rect.height}:{rect.x}:{rect.y}:exact=1"
                        if rect
                        else "pad=ceil(iw/2)*2:ceil(ih/2)*2"
                    )
                    pixel_format = (
                        "yuv444p" if rect and (rect.width % 2 or rect.height % 2) else "yuv420p"
                    )
                    arguments += (
                        "-map",
                        "0:v:0",
                        "-map",
                        "0:a?",
                        "-c:v",
                        "libx264",
                        "-preset",
                        "veryfast",
                        "-vf",
                        visual_filter,
                        "-pix_fmt",
                        pixel_format,
                        "-c:a",
                        "aac",
                        "-movflags",
                        "+faststart",
                        "-f",
                        "mp4",
                    )
                arguments += (
                    "-threads",
                    "2",
                    "-fs",
                    str(self.policy.max_derivative_bytes + 1),
                    str(output),
                )
                async with self._processes, asyncio.timeout(self.policy.transform_timeout_seconds):
                    await run_process(arguments)
                expected_ms = span.end_ms - span.start_ms if span is not None else info.duration_ms
                # FFmpeg can exit successfully after a size cap or recoverable
                # decoding failure. Publishing requires actual time coverage too.
                await self._check_coverage(
                    output, await self._probe_file(path), expected_ms, span, audio=audio
                )
                lease = await self._publish_derivative(
                    output, "audio/wav" if audio else "video/mp4"
                )
                return DerivedContent(
                    lease,
                    source.content_hash,
                    "audio_clip" if audio else "video_crop" if rect else "video_clip",
                    pixel_rect=rect,
                    time_span=span,
                )

    async def sample_frames(
        self, source: ContentObject, at_ms: tuple[int, ...], *, rect: PixelRect | None = None
    ) -> tuple[DerivedContent, ...]:
        info = await self.probe(source)
        if info.kind not in ("video", "image") or info.duration_ms is None:
            raise ValueError("Frame sampling requires temporal visual media")
        if len(at_ms) > self.policy.max_frames or any(
            value < 0 or value >= info.duration_ms for value in at_ms
        ):
            raise ValueError("Frame sample positions exceed source bounds or the frame limit")
        if rect is not None:
            self._validate_rect(rect, info.width, info.height)
        if info.kind == "image":
            return await self._sample_animation(source, at_ms, rect)
        result: list[DerivedContent] = []
        try:
            async with self.content.protect(source) as path:
                with tempfile.TemporaryDirectory(
                    prefix="nous-media-", dir=self.content.staging
                ) as temporary:
                    for ordinal, value in enumerate(at_ms):
                        output = Path(temporary) / f"frame-{ordinal}.png"
                        arguments = self._base() + (
                            "-ss",
                            str(value / 1000),
                            "-i",
                            str(path),
                            "-frames:v",
                            "1",
                        )
                        if rect is not None:
                            arguments += (
                                "-vf",
                                f"crop={rect.width}:{rect.height}:{rect.x}:{rect.y}",
                            )
                        arguments += ("-threads", "1", str(output))
                        async with (
                            self._processes,
                            asyncio.timeout(self.policy.transform_timeout_seconds),
                        ):
                            await run_process(arguments)
                        lease = await self._publish_derivative(output, "image/png")
                        result.append(
                            DerivedContent(
                                lease,
                                source.content_hash,
                                "sampled_frame",
                                pixel_rect=rect,
                                at_ms=value,
                            )
                        )
            return tuple(result)
        except BaseException:
            for derivative in result:
                await self.content.release(derivative.lease)
            raise

    async def _sample_animation(
        self, source: ContentObject, positions: tuple[int, ...], rect: PixelRect | None
    ) -> tuple[DerivedContent, ...]:
        from .animation import decode_animation_frames

        result: list[DerivedContent] = []
        try:
            async with self.content.protect(source) as path:
                with tempfile.TemporaryDirectory(
                    prefix="nous-animation-", dir=self.content.staging
                ) as temporary:
                    directory = Path(temporary)
                    async with self._processes:
                        await anyio.to_thread.run_sync(
                            decode_animation_frames,
                            self.policy.max_pixels,
                            self.policy.max_frames,
                            path,
                            directory,
                            positions,
                            rect,
                        )
                    for value in positions:
                        lease = await self._publish_derivative(
                            directory / f"{value}.png", "image/png"
                        )
                        result.append(
                            DerivedContent(
                                lease,
                                source.content_hash,
                                "sampled_frame",
                                pixel_rect=rect,
                                at_ms=value,
                            )
                        )
            return tuple(result)
        except BaseException:
            for item in result:
                await self.content.release(item.lease)
            raise
