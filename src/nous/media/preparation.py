"""Prepare actual CAS-backed text and media, preserving every selected batch."""

from __future__ import annotations

import asyncio
import hashlib
import json
import tempfile
from collections.abc import Sequence
from contextvars import ContextVar
from dataclasses import replace
from pathlib import Path
from typing import cast
from weakref import WeakValueDictionary

from markdown_it import MarkdownIt
from pydantic import JsonValue
from pydantic_ai.messages import BinaryContent

from nous.common.errors import NousError
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
from nous.content.store import ArtifactStore, ResultStore
from nous.observability.instrumentation import get_tracer
from nous.runtime.execution import ExecutionManager

from .audiovisual import AudioVisual
from .configuration import MediaConfig
from .contracts import PreparedBatch, PreparedMedia, PreparedSource
from .documents import (
    PageData,
    inspect_image,
    inspect_pdf,
    prepare_images,
    prepare_pdf_pages,
    validate_image_selection,
)

_completed: ContextVar[list[str] | None] = ContextVar("media_preparation_results", default=None)


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
        elif header.startswith((b"\x89PNG", b"\xff\xd8\xff", b"GIF8")) or (
            header.startswith(b"RIFF") and header[8:12] == b"WEBP"
        ):
            info = await self.execution.blocking(inspect_image, path)
        elif original.media_type.startswith("text/") or original.media_type == "application/json":
            text = await self.artifacts.read_text(original.ref)
            info = {"kind": "text", "lines": len(text.splitlines()), "encoding": "utf-8"}
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
            completed = _completed.get()
            if completed is not None:
                completed.append(cached.ref.address)
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
                            content=ContentSelection(artifact=original.ref, selection=selection)
                        ),
                    ),
                ),
            ),
            outputs=outputs,
            parameters={**parameters, "cache_key": cache_key},
            coverage={"selection": selection.model_dump(mode="json")},
        )
        completed = _completed.get()
        if completed is not None:
            completed.append(result.ref.address)
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
        sources: list[PreparedSource] = []
        for source in batch.sources:
            texts: list[str] = []
            media: list[BinaryContent] = []
            if source.group is None:
                raise NousError("media.invalid_plan", "Prepared media has no content references")
            for part in source.group.parts:
                artifact = await self.artifacts.get(part.content.artifact)
                if (
                    artifact.media_type.startswith("text/")
                    or artifact.media_type == "application/json"
                ):
                    texts.append(await self.artifacts.read_selection(part.content))
                else:
                    media.append(
                        BinaryContent(
                            data=await self.artifacts.read_bytes(artifact.ref),
                            media_type=artifact.media_type,
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
    ) -> PreparedMedia:
        """Prepare every requested batch, preserving durable stage references on failure."""
        completed: list[str] = []
        # Concurrent preparations keep independent lists of committed conversion/transcription stages.
        token = _completed.set(completed)
        try:
            with get_tracer(__name__).start_as_current_span(
                "media.prepare", record_exception=False
            ):
                prepared = await self._prepare(
                    artifact,
                    selection,
                    requirements=requirements,
                    settings=settings,
                    document_mode=document_mode,
                    without_audio=without_audio,
                )
            return replace(prepared, completed_result_refs=tuple(dict.fromkeys(completed)))
        except NousError as exc:
            exc.details["completed_result_refs"] = list(dict.fromkeys(completed))
            raise
        except asyncio.CancelledError as exc:
            exc.add_note(json.dumps({"completed_result_refs": list(dict.fromkeys(completed))}))
            raise
        finally:
            _completed.reset(token)

    async def _prepare(
        self,
        artifact: Artifact | ArtifactRef | str,
        selection: SourceSelection | None = None,
        *,
        requirements: Sequence[str] = (),
        settings: MediaConfig,
        document_mode: str = "auto",
        without_audio: bool = False,
    ) -> PreparedMedia:
        original = (
            artifact if isinstance(artifact, Artifact) else await self.artifacts.get(artifact)
        )
        selected = selection or SourceSelection()
        info = await self.inspect(original, settings)
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
    ) -> PreparedMedia:
        if selection.start_byte is not None or selection.end_byte is not None:
            text = await self.artifacts.read_selection(
                ContentSelection(artifact=original.ref, selection=selection)
            )
            source = PreparedSource(
                "S1",
                original.display_name,
                text=text,
                group=self._group("S1", original.display_name, original, selection),
            )
            self._split_sources((source,), 1, settings.max_inline_bytes)
            coverage: dict[str, JsonValue] = {
                "mode": "text",
                "selection": selection.model_dump(mode="json"),
            }
            return PreparedMedia(
                original,
                selection,
                (self._batch(original, (source,), selection, coverage),),
                coverage,
            )
        text = await self.artifacts.read_text(original.ref)
        lines = text.splitlines(keepends=True)
        first, last = selection.start_line or 1, selection.end_line or len(lines)
        if first > len(lines) or last > len(lines):
            raise NousError(
                "media.invalid_selection", "Text line selection is outside the artifact"
            )
        tokens = MarkdownIt().parse(text)
        boundaries = sorted(
            {first - 1, last}
            | {
                token.map[0]
                for token in tokens
                if token.level == 0 and token.map and first - 1 <= token.map[0] < last
            }
        )
        sources: list[PreparedSource] = []
        for start, end in zip(boundaries, boundaries[1:], strict=False):
            part = "".join(lines[start:end])
            if not part:
                continue
            positions = [(start, end)]
            if len(part.encode("utf-8")) > settings.max_inline_bytes:
                positions = [(index, index + 1) for index in range(start, end)]
            for lower, upper in positions:
                sources.append(
                    PreparedSource(
                        key=f"S1:L{lower + 1}-{upper}",
                        name=original.display_name,
                        text="".join(lines[lower:upper]),
                        group=self._group(
                            f"S1:L{lower + 1}-{upper}",
                            original.display_name,
                            original,
                            SourceSelection(start_line=lower + 1, end_line=upper),
                        ),
                        metadata={"representation": "original", "line_base": 1},
                    )
                )
        groups = self._split_sources(sources, max(1, len(sources)), settings.max_inline_bytes)
        batches = tuple(
            self._batch(
                original,
                group,
                SourceSelection(
                    start_line=cast(InputGroup, group[0].group)
                    .parts[0]
                    .content.selection.start_line,
                    end_line=cast(InputGroup, group[-1].group).parts[0].content.selection.end_line,
                ),
                {
                    "mode": "text",
                    "line_base": 1,
                    "start_line": cast(InputGroup, group[0].group)
                    .parts[0]
                    .content.selection.start_line,
                    "end_line": cast(InputGroup, group[-1].group)
                    .parts[0]
                    .content.selection.end_line,
                },
            )
            for group in groups
        )
        return PreparedMedia(
            original,
            selection,
            batches,
            {
                "mode": "text",
                "start_line": first,
                "end_line": last,
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
    ) -> PreparedMedia:
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
            return PreparedMedia(
                original,
                selection,
                (self._batch(original, (source,), selection, coverage),),
                coverage,
            )
        await self.execution.blocking(
            self.artifacts.content.temporary.mkdir, parents=True, exist_ok=True
        )
        workspace = await self.execution.blocking(
            tempfile.TemporaryDirectory, prefix="images-", dir=self.artifacts.content.temporary
        )
        try:
            frames = await self.execution.blocking(
                prepare_images,
                path,
                selection,
                settings.image.max_side,
                settings.video.sample_interval_ms,
                Path(workspace.name),
            )
            sources: list[PreparedSource] = []
            for frame in frames:
                data = await self.execution.blocking(frame.path.read_bytes)
                derivatives = await self._derive(
                    original,
                    frame.selection,
                    "media.prepare.image",
                    {
                        "image": OutputContent(
                            data=data,
                            media_type=frame.media_type,
                            display_name=f"frame-{frame.metadata['frame']}.png",
                        ),
                    },
                    frame.metadata,
                )
                sources.append(
                    PreparedSource(
                        key=f"S1:F{frame.metadata['frame']}",
                        name=original.display_name,
                        group=self._group(
                            f"S1:F{frame.metadata['frame']}",
                            original.display_name,
                            derivatives["image"],
                            representation="normalized_image",
                            origin=original,
                            origin_selection=frame.selection,
                        ),
                        metadata={
                            **frame.metadata,
                            "representation": "normalized_image",
                            "byte_length": len(data),
                        },
                    )
                )
        finally:
            await self.execution.blocking(workspace.cleanup)
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
        return PreparedMedia(
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
    ) -> PreparedMedia:
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
            return PreparedMedia(
                original,
                selection,
                (self._batch(original, (source,), selection, coverage),),
                coverage,
            )
        visual = mode != "text"
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
            return await self._render_pdf(original, selection, settings, path, info, pages, visual)

    async def _render_pdf(
        self,
        original: Artifact,
        selection: SourceSelection,
        settings: MediaConfig,
        path: Path,
        info: dict[str, JsonValue],
        pages: tuple[int, ...],
        visual: bool,
    ) -> PreparedMedia:
        """Read cached pages before rendering missing pages for this source."""
        parameters: dict[str, JsonValue] = {
            "tool": info["tool"],
            "version": info["version"],
            "render_dpi": settings.pdf.render_dpi,
            "mode": "visual" if visual else "text_only",
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
                        prepare_pdf_pages, path, tuple(missing), settings.pdf.render_dpi, visual
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
                            media_type="image/png",
                            display_name=f"page-{page.page}.png",
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
                        media=(BinaryContent(data=page.image, media_type="image/png"),)
                        if page.image is not None
                        else (),
                        group=InputGroup(
                            key=f"S1:P{page.page}",
                            name=f"{original.display_name} page {page.page}",
                            parts=tuple(
                                InputPart(
                                    content=ContentSelection(artifact=derivative.ref),
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
        return PreparedMedia(
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
        derivatives = await self._derive(
            original,
            selection,
            "media.prepare.audio",
            {
                "audio": OutputContent(
                    data=data, media_type="audio/wav", display_name="segment.wav"
                ),
            },
            {"tool": "FFmpeg", "version": version, "sample_rate": 16000, "channels": 1},
        )
        audio = derivatives["audio"]
        key = f"S1:A{selection.start_ms}-{selection.end_ms}"
        return PreparedSource(
            key,
            original.display_name,
            media=(BinaryContent(data=data, media_type="audio/wav"),),
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
    ) -> PreparedMedia:
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
        times = await av.frame_times(path) if is_video else ()
        selected_frames: list[tuple[int, int, int]] = []
        next_sample = start
        for index, actual in times:
            if actual >= next_sample and actual < end:
                selected_frames.append((index, next_sample, actual))
                next_sample = actual + settings.video.sample_interval_ms
        async with av.frames(
            path,
            tuple(index for index, _, _ in selected_frames),
            settings.image.max_side,
            self.artifacts.content.temporary,
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
                    data = await self.execution.blocking(frame_files[index].read_bytes)
                    frame_selection = SourceSelection(
                        start_ms=actual, end_ms=min(actual + 1, end), frame=index
                    )
                    derivatives = await self._derive(
                        original,
                        frame_selection,
                        "media.prepare.frame",
                        {
                            "frame": OutputContent(
                                data=data,
                                media_type="image/png",
                                display_name=f"frame-{actual}ms.png",
                            ),
                        },
                        {
                            "tool": "FFmpeg",
                            "version": info["version"],
                            "requested_time_ms": requested,
                            "actual_time_ms": actual,
                            "max_side": settings.image.max_side,
                        },
                    )
                    sources.append(
                        PreparedSource(
                            f"S1:F{actual}",
                            f"{original.display_name} at {actual} ms",
                            media=(BinaryContent(data=data, media_type="image/png"),),
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
                    data = await av.audio(path, lower, upper)
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
            return PreparedMedia(
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
