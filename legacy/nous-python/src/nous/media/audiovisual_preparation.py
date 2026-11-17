"""Deterministic audio and video representations."""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Sequence
from pathlib import Path
from typing import cast

from pydantic import JsonValue
from pydantic_ai.messages import BinaryContent

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ContentSelection,
    OutputContent,
    SourceSelection,
)
from nous.runtime.execution import ExecutionManager

from .audiovisual import AudioVisual
from .configuration import MediaConfig
from .contracts import (
    MediaPlan,
    PlannedConversion,
    PlannedPart,
    PreparedBatch,
    PreparedSource,
)
from .derivatives import MediaDerivatives
from .documents import (
    image_extension,
    transcode_image,
)
from .planning import (
    audio_format,
    image_format,
    make_batch,
    make_group,
    media_interval,
)


class AudioVisualPreparation:
    """Plan and materialize selected audio and video intervals."""

    def __init__(
        self, artifacts: ArtifactStore, execution: ExecutionManager, derivatives: MediaDerivatives
    ) -> None:
        self.artifacts, self.execution, self.derivatives = artifacts, execution, derivatives

    async def _audio_source(
        self,
        original: Artifact,
        selection: SourceSelection,
        convert: Callable[[], Awaitable[bytes]],
        settings: MediaConfig,
        requirements: Sequence[str],
        version: str,
    ) -> PreparedSource:
        target_mime = audio_format(requirements, settings)
        extension = {"audio/wav": "wav", "audio/mpeg": "mp3", "audio/flac": "flac"}[target_mime]

        async def make_output() -> dict[str, OutputContent]:
            return {
                "audio": OutputContent(
                    data=await convert(),
                    media_type=target_mime,
                    display_name=f"segment.{extension}",
                )
            }

        derivatives = await self.derivatives.get_or_create(
            original,
            selection,
            "media.prepare.audio",
            {
                "tool": "FFmpeg",
                "version": version,
                "sample_rate": 16000,
                "channels": 1,
                "target_media_type": target_mime,
            },
            make_output,
        )
        audio = derivatives["audio"]
        data = await self.artifacts.read_bytes(audio.ref, max_bytes=settings.max_inline_bytes)
        key = f"S1:A{selection.start_ms}-{selection.end_ms}"
        return PreparedSource(
            key,
            original.display_name,
            media=(BinaryContent(data=data, media_type=target_mime),),
            group=make_group(
                key,
                original.display_name,
                audio,
                representation="audio",
                origin=original,
                origin_selection=selection,
            ),
            metadata={"representation": "audio"},
        )

    def plan_native_video(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        info: dict[str, JsonValue],
        requirements: Sequence[str],
        without_audio: bool,
    ) -> MediaPlan:
        start, end = media_interval(info, selection)
        direct = (
            start == 0
            and end == info["duration_ms"]
            and original.media_type in requirements
            and (not without_audio or not info["has_audio"])
        )
        if not direct and "video/mp4" not in requirements:
            raise NousError(
                "media.format_unavailable",
                "Selected video intervals require an accepted MP4 representation",
            )
        coverage: dict[str, JsonValue] = {
            "mode": "native_video",
            "start_ms": start,
            "end_ms": end,
            "audio": "excluded"
            if without_audio and info["has_audio"]
            else "included"
            if info["has_audio"]
            else "absent",
            "sampling": "provider",
            "batches": 1,
        }
        if direct:
            coverage["sent_duration_ms"] = int(cast(int, info["duration_ms"]))
            size = int(cast(int, info["byte_length"]))
            if size > settings.max_inline_bytes:
                raise NousError(
                    "media.input_too_large",
                    "Native video exceeds the inline byte limit; select a shorter interval",
                    details={"bytes": size, "max_bytes": settings.max_inline_bytes},
                )
            key = f"S1:V{start}-{end}"
            source = PreparedSource(
                key,
                original.display_name,
                group=make_group(
                    key, original.display_name, original, representation="native_video"
                ),
                metadata={"representation": "native_video", "byte_length": size},
            )
            batch = make_batch(original, (source,), selection, coverage)
        else:
            recipe = PlannedConversion(
                original,
                selection,
                tuple(requirements),
                settings,
                "auto",
                without_audio,
                (
                    PlannedPart(
                        ContentSelection(artifact=original.ref, selection=selection),
                        original.media_type,
                        "native_video",
                        "video/mp4",
                        {"operation": "video_interval"},
                        info,
                    ),
                ),
            )
            batch = PreparedBatch((), selection, coverage, conversion=recipe)
        return MediaPlan(original, selection, (batch,), coverage)

    async def _native_video(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        path: Path,
        info: dict[str, JsonValue],
        requirements: Sequence[str],
        without_audio: bool,
    ) -> MediaPlan:
        plan = self.plan_native_video(
            original, selection, settings, info, requirements, without_audio
        )
        if plan.batches[0].conversion is None:
            return plan
        start, end = cast(int, plan.coverage["start_ms"]), cast(int, plan.coverage["end_ms"])
        interval = SourceSelection(start_ms=start, end_ms=end)
        av = AudioVisual(settings, self.execution)
        parameters: dict[str, JsonValue] = {
            "tool": "FFmpeg",
            "version": await av.version(),
            "without_audio": without_audio,
            "target_media_type": "video/mp4",
            "video_codec": "libx264",
            "audio_codec": "aac" if info["has_audio"] and not without_audio else None,
            "crf": 20,
            "preset": "veryfast",
        }
        cache_key = self.derivatives.key(original, interval, "media.prepare.video", parameters)
        derivatives = await self.derivatives.cached("media.prepare.video", cache_key)
        if derivatives is None:
            data = await av.video(path, start, end, without_audio=without_audio)
            if len(data) > settings.max_inline_bytes:
                raise NousError(
                    "media.input_too_large",
                    "Selected native video exceeds the inline byte limit",
                    details={"bytes": len(data), "max_bytes": settings.max_inline_bytes},
                )
            derivatives = await self.derivatives.derive(
                original,
                interval,
                "media.prepare.video",
                {
                    "video": OutputContent(
                        data=data, media_type="video/mp4", display_name="segment.mp4"
                    ),
                },
                parameters,
            )
        artifact = derivatives["video"]
        async with self.artifacts.borrow_local(artifact.ref) as prepared_path:
            sent_info = await av.inspect(prepared_path)
        coverage = {**plan.coverage, "sent_duration_ms": sent_info["duration_ms"]}
        size = (await self.artifacts.blob(artifact.ref)).byte_length
        key = f"S1:V{start}-{end}"
        source = PreparedSource(
            key,
            original.display_name,
            group=make_group(
                key,
                original.display_name,
                artifact,
                representation="native_video",
                origin=original,
                origin_selection=interval,
            ),
            metadata={"representation": "native_video", "byte_length": size},
        )
        batch = make_batch(original, (source,), interval, coverage)
        return MediaPlan(original, selection, (batch,), coverage)

    async def prepare(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        path: Path,
        info: dict[str, JsonValue],
        requirements: Sequence[str],
        without_audio: bool,
    ) -> MediaPlan:
        """Partition the interval, recording timed video samples and each batch's audio representation."""
        av = AudioVisual(settings, self.execution)
        start, end = media_interval(info, selection)
        is_video = info["kind"] == "video"
        if is_video and any(value.startswith("video/") for value in requirements):
            return await self._native_video(
                original, selection, settings, path, info, requirements, without_audio
            )
        has_audio = bool(info["has_audio"])
        streams = cast(list[dict[str, JsonValue]], info["streams"])
        origin_ms = round(
            float(
                str(
                    next(
                        (
                            stream.get("start_time", 0)
                            for stream in streams
                            if stream.get("codec_type") == "video"
                        ),
                        0,
                    )
                )
            )
            * 1000
        )
        times = await av.frame_times(path, start, end, origin_ms) if is_video else ()
        selected_frames: list[tuple[int, int, int]] = []
        next_sample = start
        for index, actual in times:
            if actual >= next_sample and actual < end:
                selected_frames.append((index, next_sample, actual))
                next_sample = actual + settings.video.sample_interval_ms
        target_mime = image_format(requirements, settings) if is_video else "image/png"
        frame_cache: dict[int, dict[str, Artifact]] = {}
        frame_parameters: dict[int, dict[str, JsonValue]] = {}
        for index, requested, actual in selected_frames:
            frame_selection = SourceSelection(start_ms=actual, end_ms=min(actual + 1, end))
            parameters: dict[str, JsonValue] = {
                "tool": "FFmpeg",
                "version": info["version"],
                "source_media_type": info["media_type"],
                "requested_time_ms": requested,
                "actual_time_ms": actual,
                "max_side": settings.image.max_side,
                "target_media_type": target_mime,
                "quality": settings.image.quality,
                "jpeg_background": settings.image.jpeg_background,
            }
            frame_parameters[index] = parameters
            cached = await self.derivatives.cached(
                "media.prepare.frame",
                self.derivatives.key(original, frame_selection, "media.prepare.frame", parameters),
            )
            if cached is not None:
                frame_cache[index] = cached
        async with av.frames(
            path,
            tuple(index for index, _, _ in selected_frames if index not in frame_cache),
            settings.image.max_side,
            self.artifacts.content.temporary,
            start,
        ) as frame_files:
            span_ms = max(1, round(settings.audio.max_seconds_per_request * 1000))
            if is_video:
                span_ms = min(
                    span_ms,
                    settings.video.sample_interval_ms * settings.video.max_frames_per_request,
                )
            batches: list[PreparedBatch] = []
            for lower in range(start, end, span_ms):
                upper = min(lower + span_ms, end)
                part_selection = SourceSelection(start_ms=lower, end_ms=upper)
                sources: list[PreparedSource] = []
                frame_coverage: list[JsonValue] = []
                for index, requested, actual in selected_frames:
                    if not lower <= actual < upper:
                        continue
                    cached = frame_cache.get(index)
                    data = (
                        await self.artifacts.read_bytes(cached["frame"].ref)
                        if cached
                        else await self.execution.blocking(frame_files[index].read_bytes)
                    )
                    if cached is None and target_mime != "image/png":
                        data = await self.execution.blocking(
                            transcode_image,
                            data,
                            target_mime,
                            settings.image.quality,
                            settings.image.jpeg_background,
                        )
                    frame_selection = SourceSelection(start_ms=actual, end_ms=min(actual + 1, end))
                    derivatives = cached or await self.derivatives.derive(
                        original,
                        frame_selection,
                        "media.prepare.frame",
                        {
                            "frame": OutputContent(
                                data=data,
                                media_type=target_mime,
                                display_name=f"frame-{actual}ms.{image_extension(target_mime)}",
                            ),
                        },
                        frame_parameters[index],
                    )
                    if cached is None:
                        data = await self.artifacts.read_bytes(derivatives["frame"].ref)
                    sources.append(
                        PreparedSource(
                            f"S1:F{actual}",
                            f"{original.display_name} at {actual} ms",
                            media=(BinaryContent(data=data, media_type=target_mime),),
                            group=make_group(
                                f"S1:F{actual}",
                                original.display_name,
                                derivatives["frame"],
                                representation="sampled_frame",
                                origin=original,
                                origin_selection=frame_selection,
                            ),
                            metadata={"representation": "sampled_frame", "actual_time_ms": actual},
                        )
                    )
                    frame_coverage.append(
                        {"requested_time_ms": requested, "actual_time_ms": actual}
                    )
                if has_audio and not without_audio:

                    async def convert_audio(lower: int = lower, upper: int = upper) -> bytes:
                        return await av.audio(
                            path, lower, upper, audio_format(requirements, settings)
                        )

                    sources.append(
                        await self._audio_source(
                            original,
                            part_selection,
                            convert_audio,
                            settings,
                            requirements,
                            str(info["version"]),
                        )
                    )
                if not sources:
                    continue
                # A batch's audio is kept with all its frames; oversize requests fail explicitly.
                total = sum(
                    len((source.text or "").encode()) + sum(len(part.data) for part in source.media)
                    for source in sources
                )
                if total > settings.max_inline_bytes:
                    raise NousError(
                        "media.input_too_large",
                        "Audio/video batch exceeds the inline byte limit; reduce batch duration or image size",
                    )
                coverage: dict[str, JsonValue] = {
                    "mode": "sampled" if is_video else "audio",
                    "start_ms": lower,
                    "end_ms": upper,
                    "frames": frame_coverage,
                    "audio": "excluded"
                    if has_audio and without_audio
                    else str(sources[-1].metadata.get("representation"))
                    if has_audio
                    else "absent",
                }
                batches.append(make_batch(original, sources, part_selection, coverage))
            if not batches:
                raise NousError("media.empty_selection", "No media was prepared for this selection")
            return MediaPlan(
                original,
                selection,
                tuple(batches),
                {
                    "mode": "sampled" if is_video else "audio",
                    "start_ms": start,
                    "end_ms": end,
                    "frames": [
                        {"requested_time_ms": requested, "actual_time_ms": actual}
                        for _, requested, actual in selected_frames
                    ],
                    "audio": "excluded"
                    if has_audio and without_audio
                    else "processed"
                    if has_audio
                    else "absent",
                    "batches": len(batches),
                },
            )
