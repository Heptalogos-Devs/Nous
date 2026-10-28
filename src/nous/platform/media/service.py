"""Pillow and FFmpeg media preparation over retained immutable originals."""

from __future__ import annotations

import asyncio
import math
import tempfile
from dataclasses import dataclass
from pathlib import Path

import anyio
from nous_wave.contracts.media import MediaInfo, PixelRect, TimeSpan
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
    nb_frames: str | None = None
    sample_rate: str | None = None
    channels: int | None = None
    disposition: dict[str, int] = Field(default_factory=dict)


class _Format(BaseModel):
    model_config = ConfigDict(extra="ignore")
    duration: str | None = None


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
        ffmpeg: str = "ffmpeg",
        ffprobe: str = "ffprobe",
        max_pixels: int = 40_000_000,
        max_frames: int = 256,
        max_derivative_bytes: int = 128 * 1024 * 1024,
        process_concurrency: int = 2,
    ):
        self.content = content
        self.ffmpeg = ffmpeg
        self.ffprobe = ffprobe
        self.max_pixels = max_pixels
        self.max_frames = max_frames
        self.max_derivative_bytes = max_derivative_bytes
        self._processes = asyncio.Semaphore(process_concurrency)

    def _check_image(self, image: Image.Image) -> None:
        if image.width * image.height > self.max_pixels:
            raise ValueError("Decoded image exceeds the instance pixel limit")
        if getattr(image, "n_frames", 1) > self.max_frames:
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
            async with self._processes, asyncio.timeout(30):
                output = await run_process(
                    (
                        self.ffprobe,
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
            probe = _Probe.model_validate_json(output.stdout)
            video = next(
                (
                    stream
                    for stream in probe.streams
                    if stream.codec_type == "video" and stream.disposition.get("attached_pic") != 1
                ),
                None,
            )
            audio = next((stream for stream in probe.streams if stream.codec_type == "audio"), None)
            durations: list[float] = []
            for value in [probe.format.duration, *(stream.duration for stream in probe.streams)]:
                if value is not None:
                    try:
                        seconds = float(value)
                    except ValueError:
                        continue
                    if math.isfinite(seconds) and seconds >= 0:
                        durations.append(seconds)
            duration_ms = round(max(durations) * 1000) if durations else None
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
        if (await anyio.to_thread.run_sync(path.stat)).st_size > self.max_derivative_bytes:
            raise ValueError("Prepared media exceeds the derivative size limit")
        return await self.content.publish_file(path, media_type)

    async def crop_image(self, source: ContentObject, rect: PixelRect) -> DerivedContent:
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
            self.ffmpeg,
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
            info.kind not in ("audio", "video")
            or info.duration_ms is None
            or span.end_ms > info.duration_ms
        ):
            raise ValueError("Time span is outside the source media duration")
        return await self._transcode(source, info, span, None, audio_only)

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
                arguments += ("-threads", "2", "-fs", str(self.max_derivative_bytes), str(output))
                async with self._processes, asyncio.timeout(120):
                    await run_process(arguments)
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
        if len(at_ms) > self.max_frames or any(
            value < 0 or value >= info.duration_ms for value in at_ms
        ):
            raise ValueError("Frame sample positions exceed source bounds or the frame limit")
        if rect is not None:
            self._validate_rect(rect, info.width, info.height)
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
                        async with self._processes, asyncio.timeout(60):
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
