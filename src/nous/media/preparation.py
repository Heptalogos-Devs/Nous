"""Prepare actual CAS-backed text and media, preserving every selected batch."""

from __future__ import annotations

import asyncio
import hashlib
import json
import shutil
import tempfile
from collections.abc import AsyncGenerator, Sequence
from contextlib import asynccontextmanager
from contextvars import ContextVar
from dataclasses import replace
from pathlib import Path
from typing import cast
from weakref import WeakValueDictionary

from PIL import Image, UnidentifiedImageError
from pydantic import JsonValue
from pydantic_ai.messages import BinaryContent

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    OriginSlice,
    OutputContent,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.runtime.execution import ExecutionManager

from .audiovisual import AudioVisual
from .configuration import MediaConfig
from .contracts import (
    MediaPlan,
    PlannedConversion,
    PlannedPart,
    PreparedBatch,
    PreparedSource,
    TextMapping,
    TextRepresentation,
)
from .documents import (
    PageData,
    extract_pdf_pages,
    image_extension,
    inspect_image,
    inspect_pdf,
    prepare_images,
    prepare_pdf_pages,
    transcode_image,
    validate_image_selection,
)

_completed: ContextVar[list[str] | None] = ContextVar("media_preparation_results", default=None)


def _track_completed(reference: str) -> None:
    completed = _completed.get()
    if completed is not None:
        completed.append(reference)
    from nous.common.operations import record_completed

    record_completed(reference)


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


class MediaPreparer:
    """Materialize precise CAS-backed representations and retain completed conversion stages."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        results: ResultStore,
        execution: ExecutionManager,
    ) -> None:
        self.artifacts, self.results, self.execution = artifacts, results, execution
        # PDFium serializes its native work; keep one preparation owner per source
        # so overlapping requests can reuse committed pages instead of rendering them again.
        self._pdf_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self._conversion_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()

    @staticmethod
    def capabilities(settings: MediaConfig) -> dict[str, JsonValue]:
        Image.init()
        formats = {"image/png": "PNG", "image/jpeg": "JPEG", "image/webp": "WEBP"}
        executables: dict[str, JsonValue] = {}
        for name, configured in (
            ("ffmpeg", settings.ffmpeg_path),
            ("ffprobe", settings.ffprobe_path),
        ):
            resolved = shutil.which(configured or name)
            executables[name] = {"path": resolved, "available": resolved is not None}
        return {
            "image_formats": [
                mime for mime in settings.image.formats if formats.get(mime) in Image.SAVE
            ],
            "executables": executables,
            "jpeg_background": settings.image.jpeg_background,
        }

    @asynccontextmanager
    async def batches(
        self, plan: MediaPlan, context: object = None
    ) -> AsyncGenerator[AsyncGenerator[PreparedBatch]]:
        """Own a sequential conversion iterator and close it when the consumer leaves."""

        async def iterate() -> AsyncGenerator[PreparedBatch]:
            for batch in plan.batches:
                yield await self.materialize(batch)

        iterator = iterate()
        try:
            yield iterator
        finally:
            await iterator.aclose()

    async def text_view(
        self, content: ContentSelection | ArtifactRef | str, *, settings: MediaConfig
    ) -> TextRepresentation:
        source = (
            content
            if isinstance(content, ContentSelection)
            else ContentSelection(artifact=(await self.artifacts.get(content)).ref)
        )
        original = await self.artifacts.get(source.artifact)
        info = await self.inspect(original, settings)
        if info["kind"] == "text":
            view = await self.artifacts.text_view(source)
            return TextRepresentation(
                original.ref,
                view,
                (self._group("S1", original.display_name, original, source.selection),),
                {"mode": "text", "selection": source.selection.model_dump(mode="json")},
                mappings=(TextMapping(source, (source,)),),
            )
        if info["kind"] != "pdf":
            raise NousError(
                "media.text_representation_missing",
                "This material requires explicit transcription or language interpretation to obtain text",
            )
        parameters: dict[str, JsonValue] = {
            "source_media_type": "application/pdf",
            "tool": info["tool"],
            "version": info["version"],
            "mapping_version": 1,
        }
        key = self._derivative_key(original, source.selection, "media.text-view", parameters)
        cached = await self.results.find_cached("media.text-view", key)
        if cached is None:
            plan = await self.prepare(
                original,
                source.selection,
                requirements=("text/plain",),
                settings=settings,
                document_mode="text",
            )
            texts: list[str] = []
            inputs: list[InputGroup] = []
            mappings: list[JsonValue] = []
            offset = 0
            async with self.batches(plan) as batches:
                async for batch in batches:
                    for item in batch.sources:
                        text = item.text or ""
                        start = offset + (2 if texts else 0)
                        offset = start + len(text.encode())
                        mappings.append(
                            {
                                "start_byte": start,
                                "end_byte": offset,
                                "origins": [
                                    part.content.model_dump(mode="json")
                                    for part in item.group.parts
                                ]
                                if item.group
                                else [],
                            }
                        )
                        texts.append(text)
                    inputs.extend(batch.inputs)
            cached = await self.results.adopt(
                operation="media.text-view",
                producer={"kind": "deterministic", **parameters},
                inputs=tuple(inputs),
                outputs={
                    "text": OutputContent(
                        data="\n\n".join(texts).encode(),
                        media_type="text/plain",
                        display_name="document.txt",
                    )
                },
                parameters={**parameters, "cache_key": key},
                coverage={**plan.coverage, "text_mappings": mappings},
            )
        _track_completed(cached.ref.address)
        body = cached.outputs[0].artifact
        resolved_mappings = tuple(
            TextMapping(
                ContentSelection(
                    artifact=body,
                    selection=SourceSelection(
                        start_byte=int(cast(int, mapping["start_byte"])),
                        end_byte=int(cast(int, mapping["end_byte"])),
                    ),
                ),
                tuple(
                    ContentSelection.model_validate(origin)
                    for origin in cast(list[JsonValue], mapping["origins"])
                ),
            )
            for mapping in cast(list[dict[str, JsonValue]], cached.coverage["text_mappings"])
        )
        return TextRepresentation(
            body,
            await self.artifacts.text_view(body),
            cached.inputs,
            cached.coverage,
            (cached.ref.address,),
            resolved_mappings,
        )

    async def inspect(
        self, artifact: Artifact | ArtifactRef | str, settings: MediaConfig
    ) -> dict[str, JsonValue]:
        original = (
            artifact if isinstance(artifact, Artifact) else await self.artifacts.get(artifact)
        )
        blob = await self.artifacts.blob(original.ref)
        async with self.artifacts.borrow_local(original.ref) as path:
            return await self._inspect_local(original, blob.byte_length, path, settings)

    async def _inspect_local(
        self, original: Artifact, byte_length: int, path: Path, settings: MediaConfig
    ) -> dict[str, JsonValue]:
        with path.open("rb") as source:
            header = source.read(16)
        if header.startswith(b"%PDF-"):
            info = await self.execution.blocking(inspect_pdf, path)
        else:
            try:
                info = await self.execution.blocking(inspect_image, path)
            except UnidentifiedImageError, OSError:
                if (
                    original.media_type.startswith("text/")
                    or original.media_type == "application/json"
                ):
                    info = {"kind": "text", "encoding": "utf-8"}
                else:
                    info = await AudioVisual(settings, self.execution).inspect(path)
        return {
            **info,
            "artifact": original.ref.model_dump(mode="json"),
            "byte_length": byte_length,
            "media_type": info.get("media_type", original.media_type),
        }

    @staticmethod
    def _derivative_key(
        original: Artifact,
        selection: SourceSelection,
        operation: str,
        parameters: dict[str, JsonValue],
    ) -> str:
        return hashlib.sha256(
            json.dumps(
                {
                    "artifact": original.ref.id,
                    "source_media_type": original.media_type,
                    "selection": selection.model_dump(mode="json"),
                    "operation": operation,
                    "parameters": parameters,
                },
                sort_keys=True,
            ).encode()
        ).hexdigest()

    async def _cached_derivative(
        self, operation: str, cache_key: str
    ) -> dict[str, Artifact] | None:
        cached = await self.results.find_cached(operation, cache_key)
        if cached is not None:
            _track_completed(cached.ref.address)
            return {
                output.name: await self.artifacts.get(output.artifact) for output in cached.outputs
            }
        return None

    async def _derive(
        self,
        original: Artifact,
        selection: SourceSelection,
        operation: str,
        outputs: dict[str, OutputContent],
        parameters: dict[str, JsonValue],
    ) -> dict[str, Artifact]:
        """Reuse derivatives for the same accepted source, selection and conversion parameters."""
        cache_key = self._derivative_key(original, selection, operation, parameters)
        cached = await self._cached_derivative(operation, cache_key)
        if cached is not None:
            return cached
        result = await self.results.adopt(
            operation=operation,
            producer={"kind": "deterministic", **parameters},
            inputs=(
                InputGroup(
                    key="S1",
                    name=original.display_name,
                    parts=(
                        InputPart(
                            content=ContentSelection(artifact=original.ref, selection=selection),
                            source_media_type=original.media_type,
                            media_type=original.media_type,
                        ),
                    ),
                ),
            ),
            outputs=outputs,
            parameters={**parameters, "cache_key": cache_key},
            coverage={"selection": selection.model_dump(mode="json")},
        )
        _track_completed(result.ref.address)
        return {output.name: await self.artifacts.get(output.artifact) for output in result.outputs}

    @staticmethod
    def _group(
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
                        ),
                    )
                    if origin is not None
                    else (),
                ),
            ),
        )

    @staticmethod
    def _batch(
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
            tuple(source.group for source in plans if source.group is not None),
            selection,
            coverage,
        )

    async def materialize(self, batch: PreparedBatch) -> PreparedBatch:
        if batch.conversion is not None:
            recipe = batch.conversion
            completed: list[str] = []
            token = _completed.set(completed)
            try:
                converted = await self._prepare(
                    recipe.artifact,
                    recipe.selection,
                    requirements=recipe.requirements,
                    settings=recipe.settings,
                    document_mode=recipe.document_mode,
                    without_audio=recipe.without_audio,
                )
                materialized = [await self.materialize(item) for item in converted.batches]
                actual_sources = tuple(source for item in materialized for source in item.sources)
                if batch.source_prefix != "S1":
                    actual_sources = tuple(
                        replace(
                            source,
                            key=batch.source_prefix + source.key[2:],
                            group=source.group.model_copy(
                                update={"key": batch.source_prefix + source.key[2:]}
                            )
                            if source.group
                            else None,
                        )
                        for source in actual_sources
                    )
                return replace(
                    batch,
                    sources=actual_sources,
                    inputs=tuple(source.group for source in actual_sources if source.group),
                    conversion=None,
                    coverage=converted.coverage,
                    completed_result_refs=tuple(dict.fromkeys(completed)),
                )
            except NousError as error:
                error.details["completed_result_refs"] = list(dict.fromkeys(completed))
                raise
            finally:
                _completed.reset(token)
        sources: list[PreparedSource] = []
        for source in batch.sources:
            texts: list[str] = []
            media: list[BinaryContent] = []
            if source.group is None:
                raise NousError("media.invalid_plan", "Prepared media has no content references")
            for part in source.group.parts:
                artifact = await self.artifacts.get(part.content.artifact)
                mime = part.media_type or artifact.media_type
                if mime.startswith("text/") or mime == "application/json":
                    texts.append(await self.artifacts.read_selection(part.content))
                else:
                    media.append(
                        BinaryContent(
                            data=await self.artifacts.read_bytes(artifact.ref),
                            media_type=mime,
                        )
                    )
            sources.append(
                replace(source, text="\n".join(texts) if texts else None, media=tuple(media))
            )
        return replace(batch, sources=tuple(sources))

    @staticmethod
    def _split_sources(
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

    async def prepare(
        self,
        artifact: Artifact | ArtifactRef | str,
        selection: SourceSelection | None = None,
        *,
        requirements: Sequence[str] = (),
        settings: MediaConfig,
        document_mode: str = "auto",
        without_audio: bool = False,
    ) -> MediaPlan:
        """Plan bounded conversion batches without publishing or decoding derivatives."""
        original = (
            artifact if isinstance(artifact, Artifact) else await self.artifacts.get(artifact)
        )
        selected = selection or SourceSelection()
        info = await self.inspect(original, settings)
        kind = str(info["kind"])
        _validate_selection(kind, selected)
        if kind == "text":
            return await self._text(original, selected, settings)
        mime = str(info["media_type"])
        # Actual identified MIME follows identity references through materialization.
        original = original.model_copy(update={"media_type": mime})
        selections: list[SourceSelection] = []
        if kind == "pdf":
            pages = selected.pages or tuple(range(1, int(cast(int, info["pages"])) + 1))
            if any(page > int(cast(int, info["pages"])) for page in pages):
                raise NousError(
                    "media.invalid_selection", "PDF page selection exceeds the document"
                )
            native = (
                document_mode == "auto"
                and "application/pdf" in requirements
                and not selected.pages
                and int(cast(int, info["byte_length"])) <= settings.max_inline_bytes
            )
            selections = (
                [selected]
                if native
                else [
                    SourceSelection(
                        pages=pages[offset : offset + settings.pdf.max_pages_per_request]
                    )
                    for offset in range(0, len(pages), settings.pdf.max_pages_per_request)
                ]
            )
            target = (
                "application/pdf"
                if document_mode == "auto" and "application/pdf" in requirements
                else self._image_format(requirements, settings)
                if document_mode != "text"
                else "text/plain"
            )
        elif kind in {"audio", "video"}:
            start, end = (
                selected.start_ms or 0,
                selected.end_ms or int(cast(int, info["duration_ms"])),
            )
            if not 0 <= start < end <= int(cast(int, info["duration_ms"])):
                raise NousError(
                    "media.invalid_selection", "Time selection exceeds the media duration"
                )
            span = max(1, round(settings.audio.max_seconds_per_request * 1000))
            if kind == "video":
                span = min(
                    span, settings.video.sample_interval_ms * settings.video.max_frames_per_request
                )
            selections = [
                SourceSelection(start_ms=lower, end_ms=min(lower + span, end))
                for lower in range(start, end, span)
            ]
            target = self._image_format(requirements, settings) if kind == "video" else "audio/wav"
        else:
            validate_image_selection(selected, int(cast(int, info["frames"])))
            selections = [selected]
            if selected.frame is not None and int(cast(int, info["frames"])) > 1:
                chosen_frame = cast(list[dict[str, int]], info["frame_times"])[selected.frame]
                if (
                    chosen_frame["start_ms"] + max(1, chosen_frame["duration_ms"])
                    <= (selected.start_ms or 0)
                    or selected.end_ms is not None
                    and chosen_frame["start_ms"] >= selected.end_ms
                ):
                    raise NousError(
                        "media.invalid_selection",
                        "Selected image frame and time interval do not overlap",
                    )
            if int(cast(int, info["frames"])) > 1 and selected.frame is None:
                next_sample = selected.start_ms or 0
                selections = []
                for frame in cast(list[dict[str, int]], info["frame_times"]):
                    position, span = frame["start_ms"], max(1, frame["duration_ms"])
                    if position + span > next_sample and (
                        selected.end_ms is None or position < selected.end_ms
                    ):
                        selections.append(selected.model_copy(update={"frame": frame["frame"]}))
                        next_sample = max(
                            next_sample + settings.video.sample_interval_ms, position + span
                        )
                if not selections:
                    raise NousError(
                        "media.empty_selection", "No animation frames fall in the selected interval"
                    )
            target = (
                mime
                if mime in (requirements or ("image/png", "image/jpeg", "image/webp"))
                else self._image_format(requirements, settings)
            )
            if (
                target == "image/jpeg"
                and bool(info["has_alpha"])
                and settings.image.jpeg_background is None
            ):
                raise NousError(
                    "media.jpeg_background_required",
                    "Transparent images require an explicit JPEG background",
                )
        batches: list[PreparedBatch] = []
        for part_selection in selections:
            identity = (
                kind == "image"
                and mime == target
                and info["frames"] == 1
                and info["orientation"] == 1
                and selected.crop is None
                and max(int(cast(int, info["width"])), int(cast(int, info["height"])))
                <= settings.image.max_side
                or kind == "pdf"
                and target == "application/pdf"
                and not part_selection.pages
            )
            part = PlannedPart(
                ContentSelection(artifact=original.ref, selection=part_selection),
                mime,
                "original" if identity else kind,
                target,
                {
                    "operation": "identity" if identity else "convert",
                    "target_media_type": target,
                    "max_side": settings.image.max_side,
                    "quality": settings.image.quality,
                    "jpeg_background": settings.image.jpeg_background,
                    "render_dpi": settings.pdf.render_dpi,
                },
                info,
            )
            parts = (part,)
            if kind == "video" and bool(info.get("has_audio")) and not without_audio:
                parts += (
                    PlannedPart(
                        part.source,
                        mime,
                        "audio",
                        self._audio_format(requirements, settings),
                        {"operation": "audio", "sample_rate": 16000, "channels": 1},
                    ),
                )
            recipe = PlannedConversion(
                original,
                part_selection,
                tuple(requirements),
                settings,
                document_mode,
                without_audio,
                parts,
            )
            coverage: dict[str, JsonValue] = {
                "kind": kind,
                "selection": part_selection.model_dump(mode="json"),
            }
            batches.append(PreparedBatch((), (), part_selection, coverage, conversion=recipe))
        return MediaPlan(
            original,
            selected,
            tuple(batches),
            {"kind": kind, "selection": selected.model_dump(mode="json"), "batches": len(batches)},
        )

    @staticmethod
    def _image_format(requirements: Sequence[str], settings: MediaConfig) -> str:
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

    @staticmethod
    def _audio_format(requirements: Sequence[str], settings: MediaConfig) -> str:
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
            details={"wanted": list(requirements), "available": settings.audio.formats},
        )

    async def _prepare(
        self,
        artifact: Artifact | ArtifactRef | str,
        selection: SourceSelection | None = None,
        *,
        requirements: Sequence[str] = (),
        settings: MediaConfig,
        document_mode: str = "auto",
        without_audio: bool = False,
    ) -> MediaPlan:
        original = (
            artifact if isinstance(artifact, Artifact) else await self.artifacts.get(artifact)
        )
        selected = selection or SourceSelection()
        info = await self.inspect(original, settings)
        original = original.model_copy(update={"media_type": str(info["media_type"])})
        async with self.artifacts.borrow_local(original.ref) as path:
            kind = cast(str, info["kind"])
            _validate_selection(kind, selected)
            if kind == "text":
                return await self._text(original, selected, settings)
            if kind == "image":
                return await self._images(original, selected, settings, path, info, requirements)
            if kind == "pdf":
                return await self._pdf(
                    original, selected, settings, path, info, requirements, document_mode
                )
            return await self._av(
                original,
                selected,
                settings,
                path,
                info,
                requirements,
                without_audio,
            )

    async def _text(
        self, original: Artifact, selection: SourceSelection, settings: MediaConfig
    ) -> MediaPlan:
        view = await self.artifacts.text_view(
            ContentSelection(artifact=original.ref, selection=selection)
        )
        sources = tuple(
            PreparedSource(
                key=f"S1:C{index}",
                name=original.display_name,
                text=chunk.text,
                group=self._group(
                    f"S1:C{index}",
                    original.display_name,
                    original,
                    chunk.selection.selection,
                    representation="text",
                ),
                metadata={
                    "representation": "text",
                    "parser_profile": view.parser_profile,
                    "blocks": list[JsonValue](
                        block.model_dump(mode="json") for block in chunk.blocks
                    ),
                    "byte_length": len(chunk.text.encode("utf-8")),
                },
            )
            for index, chunk in enumerate(view.chunks(max_bytes=settings.max_inline_bytes), 1)
        )
        if not sources:
            raise NousError("media.empty_selection", "The selected text has no readable body")
        groups = self._split_sources(sources, len(sources), settings.max_inline_bytes)
        batches: list[PreparedBatch] = []
        for group in groups:
            inputs = tuple(source.group for source in group if source.group is not None)
            actual = SourceSelection(
                start_byte=inputs[0].parts[0].content.selection.start_byte,
                end_byte=inputs[-1].parts[0].content.selection.end_byte,
            )
            batches.append(
                self._batch(
                    original,
                    group,
                    actual,
                    {
                        "mode": "text",
                        "selection": actual.model_dump(mode="json"),
                        "parser_profile": view.parser_profile,
                    },
                )
            )
        return MediaPlan(
            original,
            selection,
            tuple(batches),
            {
                "mode": "text",
                "selection": selection.model_dump(mode="json"),
                "parser_profile": view.parser_profile,
                "batches": len(batches),
            },
        )

    async def _images(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        path: Path,
        info: dict[str, JsonValue],
        requirements: Sequence[str],
    ) -> MediaPlan:
        validate_image_selection(selection, cast(int, info["frames"]))
        mime = str(info["media_type"])
        if (
            info["frames"] == 1
            and info["orientation"] == 1
            and selection.crop is None
            and max(cast(int, info["width"]), cast(int, info["height"])) <= settings.image.max_side
            and mime in (requirements or ("image/png", "image/jpeg", "image/webp"))
        ):
            data = await self.artifacts.read_bytes(
                original.ref, max_bytes=settings.max_inline_bytes
            )
            source = PreparedSource(
                "S1",
                original.display_name,
                media=(BinaryContent(data=data, media_type=mime),),
                group=self._group("S1", original.display_name, original, selection),
                metadata={"representation": "original_image"},
            )
            coverage: dict[str, JsonValue] = {
                "mode": "image",
                "width": info["width"],
                "height": info["height"],
            }
            return MediaPlan(
                original,
                selection,
                (self._batch(original, (source,), selection, coverage),),
                coverage,
            )
        target = self._image_format(requirements, settings)
        parameters: dict[str, JsonValue] = {
            "tool": "Pillow",
            "version": info["version"],
            "source_media_type": mime,
            "target_media_type": target,
            "max_side": settings.image.max_side,
            "quality": settings.image.quality,
            "jpeg_background": settings.image.jpeg_background,
            "sample_interval_ms": settings.video.sample_interval_ms,
        }
        cache_key = self._derivative_key(original, selection, "media.prepare.image", parameters)
        lock = self._conversion_locks.setdefault(cache_key, asyncio.Lock())
        async with lock:
            cached = await self.results.find_cached("media.prepare.image", cache_key)
            if cached is not None:
                _track_completed(cached.ref.address)
                frame_info = cast(list[dict[str, JsonValue]], cached.parameters["frames"])
                derivatives = {
                    output.name: await self.artifacts.get(output.artifact)
                    for output in cached.outputs
                }
            else:
                await self.execution.blocking(
                    self.artifacts.content.temporary.mkdir, parents=True, exist_ok=True
                )
                workspace = await self.execution.blocking(
                    tempfile.TemporaryDirectory,
                    prefix="images-",
                    dir=self.artifacts.content.temporary,
                )
                try:
                    frames = await self.execution.blocking(
                        prepare_images,
                        path,
                        selection,
                        settings.image.max_side,
                        settings.video.sample_interval_ms,
                        Path(workspace.name),
                        target,
                        settings.image.quality,
                        settings.image.jpeg_background,
                    )
                    frame_info = [
                        {"selection": frame.selection.model_dump(mode="json"), **frame.metadata}
                        for frame in frames
                    ]
                    outputs = {
                        f"image-{index}": OutputContent(
                            data=await self.execution.blocking(frame.path.read_bytes),
                            media_type=target,
                            display_name=f"frame-{frame.metadata['frame']}.{image_extension(target)}",
                        )
                        for index, frame in enumerate(frames)
                    }
                    derived = await self.results.adopt(
                        operation="media.prepare.image",
                        producer={"kind": "deterministic", **parameters},
                        inputs=tuple(
                            self._group(
                                f"S1:F{frame['frame']}",
                                original.display_name,
                                original,
                                SourceSelection.model_validate(frame["selection"]),
                            )
                            for frame in frame_info
                        ),
                        outputs=outputs,
                        parameters={**parameters, "cache_key": cache_key, "frames": frame_info},
                        coverage={"selection": selection.model_dump(mode="json")},
                    )
                    _track_completed(derived.ref.address)
                    derivatives = {
                        output.name: await self.artifacts.get(output.artifact)
                        for output in derived.outputs
                    }
                finally:
                    await self.execution.blocking(workspace.cleanup)
        sources: list[PreparedSource] = []
        for index, frame in enumerate(frame_info):
            artifact = derivatives[f"image-{index}"]
            source_selection = SourceSelection.model_validate(frame["selection"])
            sources.append(
                PreparedSource(
                    key=f"S1:F{frame['frame']}",
                    name=original.display_name,
                    group=self._group(
                        f"S1:F{frame['frame']}",
                        original.display_name,
                        artifact,
                        representation="normalized_image",
                        origin=original,
                        origin_selection=source_selection,
                    ),
                    metadata={
                        **frame,
                        "representation": "normalized_image",
                        "byte_length": (await self.artifacts.blob(artifact.ref)).byte_length,
                    },
                )
            )
        groups = self._split_sources(
            sources, settings.video.max_frames_per_request, settings.max_inline_bytes
        )
        mode = "sampled" if int(cast(int, info["frames"])) > 1 else "image"
        batches = tuple(
            self._batch(
                original,
                group,
                selection,
                {
                    "mode": mode,
                    "frames": [source.metadata for source in group],
                },
            )
            for group in groups
        )
        return MediaPlan(
            original,
            selection,
            batches,
            {
                "mode": mode,
                "frames": [source.metadata for source in sources],
                "batches": len(batches),
            },
        )

    async def _pdf(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        path: Path,
        info: dict[str, JsonValue],
        requirements: Sequence[str],
        mode: str,
    ) -> MediaPlan:
        if mode not in {"auto", "visual", "text"}:
            raise NousError("media.invalid_mode", "Document mode must be auto, visual or text")
        count = int(cast(int, info["pages"]))
        pages = selection.pages or tuple(range(1, count + 1))
        if any(page > count for page in pages):
            raise NousError("media.invalid_selection", "PDF page selection exceeds the document")
        if (
            mode == "auto"
            and "application/pdf" in requirements
            and not selection.pages
            and (await self.artifacts.blob(original.ref)).byte_length <= settings.max_inline_bytes
        ):
            data = await self.artifacts.read_bytes(
                original.ref, max_bytes=settings.max_inline_bytes
            )
            source = PreparedSource(
                "S1",
                original.display_name,
                media=(BinaryContent(data=data, media_type="application/pdf"),),
                group=self._group("S1", original.display_name, original, selection),
                metadata={"representation": "native_pdf"},
            )
            coverage: dict[str, JsonValue] = {"mode": "native_pdf", "pages": list(pages)}
            return MediaPlan(
                original,
                selection,
                (self._batch(original, (source,), selection, coverage),),
                coverage,
            )
        visual = mode != "text"
        if mode == "auto" and "application/pdf" in requirements:
            return await self._native_pdf(original, selection, settings, path, info, pages)
        if visual and requirements and not any(mime.startswith("image/") for mime in requirements):
            raise NousError(
                "media.vision_required",
                "PDF visual preparation requires an image-capable model; choose text mode explicitly",
            )
        lock = self._pdf_locks.get(original.ref.id)
        if lock is None:
            lock = asyncio.Lock()
            self._pdf_locks[original.ref.id] = lock
        async with lock:
            return await self._render_pdf(
                original,
                selection,
                settings,
                path,
                info,
                pages,
                visual,
                self._image_format(requirements, settings) if visual else "text/plain",
            )

    async def _native_pdf(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        path: Path,
        info: dict[str, JsonValue],
        pages: tuple[int, ...],
    ) -> MediaPlan:
        parameters: dict[str, JsonValue] = {
            "tool": info["tool"],
            "version": info["version"],
            "target_media_type": "application/pdf",
        }
        selected = SourceSelection(pages=pages)
        cached = await self._cached_derivative(
            "media.prepare.document",
            self._derivative_key(original, selected, "media.prepare.document", parameters),
        )
        data = (
            await self.artifacts.read_bytes(cached["document"].ref)
            if cached
            else await self.execution.blocking(extract_pdf_pages, path, pages)
        )
        if len(data) > settings.max_inline_bytes:
            if len(pages) == 1:
                raise NousError(
                    "media.input_too_large",
                    "A native PDF page exceeds the request byte allowance",
                    details={
                        "page": pages[0],
                        "bytes": len(data),
                        "max_bytes": settings.max_inline_bytes,
                    },
                )
            middle = len(pages) // 2
            halves = [
                await self._native_pdf(original, selection, settings, path, info, part)
                for part in (pages[:middle], pages[middle:])
            ]
            return MediaPlan(
                original,
                selection,
                tuple(batch for half in halves for batch in half.batches),
                {"mode": "native_pdf", "pages": list(pages)},
            )
        derivatives = cached or await self._derive(
            original,
            selected,
            "media.prepare.document",
            {
                "document": OutputContent(
                    data=data, media_type="application/pdf", display_name="pages.pdf"
                )
            },
            parameters,
        )
        source = PreparedSource(
            f"S1:P{pages[0]}-{pages[-1]}",
            original.display_name,
            media=(BinaryContent(data=data, media_type="application/pdf"),),
            group=self._group(
                f"S1:P{pages[0]}-{pages[-1]}",
                original.display_name,
                derivatives["document"],
                representation="original_document",
                origin=original,
                origin_selection=selected,
            ),
        )
        coverage: dict[str, JsonValue] = {"mode": "native_pdf", "pages": list(pages)}
        return MediaPlan(
            original, selection, (self._batch(original, (source,), selected, coverage),), coverage
        )

    async def _render_pdf(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        path: Path,
        info: dict[str, JsonValue],
        pages: tuple[int, ...],
        visual: bool,
        target_mime: str,
    ) -> MediaPlan:
        """Read cached pages before rendering missing pages for this source."""
        parameters: dict[str, JsonValue] = {
            "tool": info["tool"],
            "version": info["version"],
            "render_dpi": settings.pdf.render_dpi,
            "mode": "visual" if visual else "text_only",
            "target_media_type": target_mime,
            "quality": settings.image.quality,
            "jpeg_background": settings.image.jpeg_background,
        }
        batches: list[PreparedBatch] = []
        for offset in range(0, len(pages), settings.pdf.max_pages_per_request):
            page_group = pages[offset : offset + settings.pdf.max_pages_per_request]
            cached_pages: dict[int, dict[str, Artifact]] = {}
            missing: list[int] = []
            for number in dict.fromkeys(page_group):
                page_selection = SourceSelection(pages=(number,))
                key = self._derivative_key(
                    original, page_selection, "media.prepare.pdf", parameters
                )
                cached = await self._cached_derivative("media.prepare.pdf", key)
                if cached is None:
                    missing.append(number)
                else:
                    cached_pages[number] = cached
            rendered: dict[int, PageData] = {}
            if missing:
                rendered = {
                    page.page: page
                    for page in await self.execution.blocking(
                        prepare_pdf_pages,
                        path,
                        tuple(missing),
                        settings.pdf.render_dpi,
                        visual,
                        target_mime,
                        settings.image.quality,
                        settings.image.jpeg_background,
                    )
                }
            sources: list[PreparedSource] = []
            for number in page_group:
                page_selection = SourceSelection(pages=(number,))
                derivatives = cached_pages.get(number)
                if derivatives is not None:
                    if len(derivatives) != (2 if visual else 1):
                        raise NousError("media.invalid_cache", "Cached PDF page is incomplete")
                    text = await self.artifacts.read_text(
                        derivatives["text"].ref, max_bytes=settings.max_inline_bytes
                    )
                    image = (
                        await self.artifacts.read_bytes(
                            derivatives["image"].ref, max_bytes=settings.max_inline_bytes
                        )
                        if visual
                        else None
                    )
                    page = PageData(number, text, image)
                else:
                    page = rendered[number]
                    content = {
                        "text": OutputContent(
                            data=page.text.encode("utf-8"),
                            media_type="text/plain",
                            display_name=f"page-{page.page}.txt",
                        )
                    }
                    if page.image is not None:
                        content["image"] = OutputContent(
                            data=page.image,
                            media_type=target_mime,
                            display_name=f"page-{page.page}.{image_extension(target_mime)}",
                        )
                    derivatives = await self._derive(
                        original, page_selection, "media.prepare.pdf", content, parameters
                    )
                    cached_pages[number] = derivatives
                if not page.text.strip() and not visual:
                    raise NousError(
                        "media.page_requires_vision",
                        "Selected PDF page has no extractable text",
                        details={"page": page.page, "artifact": original.ref.address},
                    )
                # Text and page image retain their independent body references in the result graph.
                sources.append(
                    PreparedSource(
                        key=f"S1:P{page.page}",
                        name=f"{original.display_name} page {page.page}",
                        text=page.text,
                        media=(BinaryContent(data=page.image, media_type=target_mime),)
                        if page.image is not None
                        else (),
                        group=InputGroup(
                            key=f"S1:P{page.page}",
                            name=f"{original.display_name} page {page.page}",
                            parts=tuple(
                                InputPart(
                                    content=ContentSelection(artifact=derivative.ref),
                                    media_type=derivative.media_type,
                                    source_media_type="application/pdf",
                                    representation="image"
                                    if derivative.media_type.startswith("image/")
                                    else "text",
                                    origins=(
                                        OriginSlice(
                                            origin=ContentSelection(
                                                artifact=original.ref, selection=page_selection
                                            ),
                                            transform_ref=derivative.producer_result_ref,
                                        ),
                                    ),
                                )
                                for derivative in derivatives.values()
                            ),
                        ),
                        metadata={
                            "representation": "page_image_and_text" if visual else "text_only",
                            "page": page.page,
                        },
                    )
                )
            for group in self._split_sources(
                sources, settings.pdf.max_pages_per_request, settings.max_inline_bytes
            ):
                actual = tuple(cast(int, source.metadata["page"]) for source in group)
                batches.append(
                    self._batch(
                        original,
                        group,
                        SourceSelection(pages=actual),
                        {
                            "mode": "visual" if visual else "text_only",
                            "pages": list(actual),
                        },
                    )
                )
        return MediaPlan(
            original,
            selection,
            tuple(batches),
            {
                "mode": "visual" if visual else "text_only",
                "pages": list(pages),
                "batches": len(batches),
            },
        )

    async def _audio_source(
        self,
        original: Artifact,
        selection: SourceSelection,
        data: bytes,
        settings: MediaConfig,
        requirements: Sequence[str],
        version: str,
    ) -> PreparedSource:
        target_mime = self._audio_format(requirements, settings)
        extension = {"audio/wav": "wav", "audio/mpeg": "mp3", "audio/flac": "flac"}[target_mime]
        derivatives = await self._derive(
            original,
            selection,
            "media.prepare.audio",
            {
                "audio": OutputContent(
                    data=data, media_type=target_mime, display_name=f"segment.{extension}"
                ),
            },
            {
                "tool": "FFmpeg",
                "version": version,
                "sample_rate": 16000,
                "channels": 1,
                "target_media_type": target_mime,
            },
        )
        audio = derivatives["audio"]
        key = f"S1:A{selection.start_ms}-{selection.end_ms}"
        return PreparedSource(
            key,
            original.display_name,
            media=(BinaryContent(data=data, media_type=target_mime),),
            group=self._group(
                key,
                original.display_name,
                audio,
                representation="audio",
                origin=original,
                origin_selection=selection,
            ),
            metadata={"representation": "audio"},
        )

    async def _av(
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
        start, end = (
            selection.start_ms or 0,
            selection.end_ms or int(cast(int, info["duration_ms"])),
        )
        duration = int(cast(int, info["duration_ms"]))
        if not 0 <= start < end <= duration:
            raise NousError("media.invalid_selection", "Time selection exceeds the media duration")
        is_video = info["kind"] == "video"
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
        target_mime = self._image_format(requirements, settings) if is_video else "image/png"
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
            cached = await self._cached_derivative(
                "media.prepare.frame",
                self._derivative_key(original, frame_selection, "media.prepare.frame", parameters),
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
                    derivatives = cached or await self._derive(
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
                    sources.append(
                        PreparedSource(
                            f"S1:F{actual}",
                            f"{original.display_name} at {actual} ms",
                            media=(BinaryContent(data=data, media_type=target_mime),),
                            group=self._group(
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
                    audio_parameters: dict[str, JsonValue] = {
                        "tool": "FFmpeg",
                        "version": str(info["version"]),
                        "sample_rate": 16000,
                        "channels": 1,
                        "target_media_type": self._audio_format(requirements, settings),
                    }
                    audio_cached = await self._cached_derivative(
                        "media.prepare.audio",
                        self._derivative_key(
                            original, part_selection, "media.prepare.audio", audio_parameters
                        ),
                    )
                    data = (
                        await self.artifacts.read_bytes(audio_cached["audio"].ref)
                        if audio_cached
                        else await av.audio(
                            path, lower, upper, self._audio_format(requirements, settings)
                        )
                    )
                    sources.append(
                        await self._audio_source(
                            original,
                            part_selection,
                            data,
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
                batches.append(self._batch(original, sources, part_selection, coverage))
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
