"""Pure selection, batching and encoding decisions for media plans."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import replace
from typing import cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ContentSelection,
    InputGroup,
    InputPart,
    OriginSlice,
    SourceContext,
    SourceSelection,
)

from .configuration import MediaConfig
from .contracts import (
    MediaPlan,
    PreparedBatch,
    PreparedSource,
)
from .documents import (
    validate_image_selection,
)


def _validate_selection(kind: str, selection: SourceSelection) -> None:
    """Reject coordinates that the selected media path cannot actually apply."""
    supported = {
        "text": {"start_line", "end_line", "start_byte", "end_byte"},
        "image": {"frame", "crop", "start_ms", "end_ms"},
        "pdf": {"pages"},
        "audio": {"start_ms", "end_ms"},
        "video": {"start_ms", "end_ms"},
    }
    if kind not in supported:
        raise NousError(
            "media.unsupported_format", "Unsupported media type", details={"kind": kind}
        )
    supplied = {
        name
        for name, value in selection.model_dump().items()
        if value is not None and (name != "pages" or bool(value))
    }
    invalid = sorted(supplied - supported[kind])
    if invalid:
        raise NousError(
            "media.invalid_selection",
            "The selected media type does not support these coordinates",
            details={"kind": kind, "fields": invalid},
        )


def validate_selection_bounds(info: dict[str, JsonValue], selection: SourceSelection) -> None:
    """Check selected coordinates against deterministic media inspection metadata."""
    kind = str(info["kind"])
    _validate_selection(kind, selection)
    if kind == "pdf":
        if any(page > int(cast(int, info["pages"])) for page in selection.pages):
            raise NousError("media.invalid_selection", "PDF page selection exceeds the document")
    elif kind in {"audio", "video"}:
        duration = info.get("duration_ms")
        start = selection.start_ms or 0
        end = selection.end_ms if selection.end_ms is not None else duration
        if (
            start < 0
            or end is not None
            and (not isinstance(end, int) or end <= start)
            or (isinstance(duration, int) and end is not None and end > duration)
        ):
            raise NousError("media.invalid_selection", "Time selection exceeds the media duration")
    elif kind == "image":
        frames = int(cast(int, info["frames"]))
        validate_image_selection(selection, frames)
        if selection.crop is not None:
            width, height = int(cast(int, info["width"])), int(cast(int, info["height"]))
            if int(cast(int, info["orientation"])) in {5, 6, 7, 8}:
                width, height = height, width
            if selection.crop[2] > width or selection.crop[3] > height:
                raise NousError("media.invalid_selection", "Pixel crop exceeds the image")
        if frames > 1:
            duration = int(cast(int, info["duration_ms"]))
            if (selection.start_ms is not None and selection.start_ms >= duration) or (
                selection.end_ms is not None and selection.end_ms > duration
            ):
                raise NousError(
                    "media.invalid_selection", "Time selection exceeds the image duration"
                )
            if selection.frame is not None:
                frame = cast(list[dict[str, int]], info["frame_times"])[selection.frame]
                if (
                    frame["start_ms"] + max(1, frame["duration_ms"]) <= (selection.start_ms or 0)
                    or selection.end_ms is not None
                    and frame["start_ms"] >= selection.end_ms
                ):
                    raise NousError(
                        "media.invalid_selection",
                        "Selected image frame and time interval do not overlap",
                    )


def media_interval(info: dict[str, JsonValue], selection: SourceSelection) -> tuple[int, int]:
    validate_selection_bounds(info, selection)
    end = selection.end_ms if selection.end_ms is not None else info.get("duration_ms")
    if not isinstance(end, int):
        raise NousError(
            "media.duration_unknown", "Choose an end time for media with unknown duration"
        )
    return selection.start_ms or 0, end


def make_group(
    key: str,
    name: str,
    artifact: Artifact,
    selection: SourceSelection | None = None,
    *,
    representation: str = "original",
    origin: Artifact | None = None,
    origin_selection: SourceSelection | None = None,
    media_type: str | None = None,
) -> InputGroup:
    return InputGroup(
        key=key,
        slot_key="S1",
        name=name,
        parts=(
            InputPart(
                content=ContentSelection(
                    artifact=artifact.ref, selection=selection or SourceSelection()
                ),
                representation=representation,
                media_type=media_type or artifact.media_type,
                source_media_type=origin.media_type if origin else artifact.media_type,
                origins=(
                    OriginSlice(
                        origin=ContentSelection(
                            artifact=origin.ref, selection=origin_selection or SourceSelection()
                        ),
                        transform_ref=artifact.producer_result_ref,
                        precision="atomic",
                    ),
                )
                if origin is not None
                else (),
            ),
        ),
    )


def make_batch(
    original: Artifact,
    sources: Sequence[PreparedSource],
    selection: SourceSelection,
    coverage: dict[str, JsonValue],
) -> PreparedBatch:
    # Plans keep references only; a consumer materializes one bounded batch when needed.
    plans = tuple(
        replace(
            source,
            text=None,
            media=(),
            group=source.group.model_copy(update={"key": source.key, "name": source.name})
            if source.group
            else None,
            metadata={
                **source.metadata,
                "byte_length": source.metadata.get(
                    "byte_length",
                    len((source.text or "").encode("utf-8"))
                    + sum(len(part.data) for part in source.media),
                ),
            },
        )
        for source in sources
    )
    return PreparedBatch(
        plans,
        selection,
        coverage,
    )


def split_sources(
    sources: Sequence[PreparedSource], max_items: int, max_bytes: int
) -> tuple[tuple[PreparedSource, ...], ...]:
    batches: list[tuple[PreparedSource, ...]] = []
    pending: list[PreparedSource] = []
    byte_length = 0
    for source in sources:
        size = int(
            cast(
                int,
                source.metadata.get(
                    "byte_length",
                    len((source.text or "").encode("utf-8"))
                    + sum(len(part.data) for part in source.media),
                ),
            )
        )
        if size > max_bytes:
            raise NousError(
                "media.input_too_large",
                "A prepared source exceeds the inline byte limit",
                details={"source": source.key, "bytes": size, "max_bytes": max_bytes},
            )
        if pending and (len(pending) >= max_items or byte_length + size > max_bytes):
            batches.append(tuple(pending))
            pending, byte_length = [], 0
        pending.append(source)
        byte_length += size
    if pending:
        batches.append(tuple(pending))
    return tuple(batches)


def with_source_context(plan: MediaPlan, info: dict[str, JsonValue]) -> MediaPlan:
    """Keep known original extents with each exact representation sent to a model."""
    kind = cast(str, info["kind"])

    def number(value: JsonValue | None) -> int | None:
        return value if isinstance(value, int) and not isinstance(value, bool) else None

    batches: list[PreparedBatch] = []
    for batch in plan.batches:
        sources: list[PreparedSource] = []
        for source in batch.sources:
            selections: list[SourceSelection] = []
            if source.group is not None:
                for part in source.group.parts:
                    selections.extend(
                        origin.origin.selection
                        for origin in part.origins
                        if origin.origin.artifact == plan.artifact.ref
                    )
                    if part.content.artifact == plan.artifact.ref:
                        selections.append(part.content.selection)
            if not selections:
                selections.append(batch.selection)
            actual = tuple(
                dict.fromkeys(
                    plan.selection if item.is_whole and not plan.selection.is_whole else item
                    for item in selections
                )
            )
            audio = plan.coverage.get("audio")
            context = SourceContext.model_validate(
                {
                    "kind": kind,
                    "selections": actual,
                    "duration_ms": number(info.get("duration_ms")),
                    "page_count": number(info.get("pages")),
                    "byte_length": number(info.get("byte_length")),
                    "frame_count": number(info.get("frames")),
                    "width": number(source.metadata.get("source_width", info.get("width"))),
                    "height": number(source.metadata.get("source_height", info.get("height"))),
                    "audio": audio
                    if isinstance(audio, str)
                    and audio in {"included", "excluded", "absent", "processed"}
                    else None,
                }
            )
            sources.append(
                replace(
                    source,
                    group=source.group.model_copy(update={"context": context})
                    if source.group
                    else None,
                )
            )
        batches.append(replace(batch, sources=tuple(sources)))
    return replace(plan, batches=tuple(batches))


def image_format(requirements: Sequence[str], settings: MediaConfig) -> str:
    supported = {"image/png", "image/jpeg", "image/webp"}
    available = [
        mime
        for mime in settings.image.formats
        if mime in supported and (not requirements or mime in requirements)
    ]
    if not available:
        raise NousError(
            "media.format_unavailable",
            "No supported image encoding is accepted by this route",
            details={"wanted": list(requirements), "available": list(supported)},
        )
    return available[0]


def audio_format(requirements: Sequence[str], settings: MediaConfig) -> str:
    available = [
        mime
        for mime in settings.audio.formats
        if mime in {"audio/wav", "audio/mpeg", "audio/flac"}
        and (not requirements or mime in requirements)
    ]
    if available:
        return available[0]
    if settings.audio.transcription_task:
        return "audio/wav"
    raise NousError(
        "media.audio_capability_missing",
        "Audio requires an accepted encoding or a configured transcription task",
        details={"wanted": list(requirements), "available": list(settings.audio.formats)},
    )
