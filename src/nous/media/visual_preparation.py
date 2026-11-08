"""Deterministic image and PDF representations."""

from __future__ import annotations

import asyncio
import tempfile
from collections.abc import Sequence
from pathlib import Path
from typing import cast
from weakref import WeakValueDictionary

from pydantic import JsonValue
from pydantic_ai.messages import BinaryContent

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ContentSelection,
    InputGroup,
    InputPart,
    OriginSlice,
    OutputContent,
    OutputMapping,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.runtime.execution import ExecutionManager, acquire_owned

from .configuration import MediaConfig
from .contracts import (
    MediaPlan,
    PreparedBatch,
    PreparedSource,
)
from .derivatives import MediaDerivatives, track_completed
from .documents import (
    PageData,
    extract_pdf_pages,
    image_extension,
    prepare_images,
    prepare_pdf_pages,
    validate_image_selection,
)
from .planning import (
    image_format,
    make_batch,
    make_group,
    split_sources,
)


class VisualPreparation:
    """Prepare and reuse image and PDF conversions."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        results: ResultStore,
        execution: ExecutionManager,
        derivatives: MediaDerivatives,
    ) -> None:
        self.artifacts, self.results, self.execution, self.derivatives = (
            artifacts,
            results,
            execution,
            derivatives,
        )
        # PDFium serializes native work; concurrent calls can reuse committed pages.
        self._pdf_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self._conversion_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()

    async def images(
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
                group=make_group("S1", original.display_name, original, selection),
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
                (make_batch(original, (source,), selection, coverage),),
                coverage,
            )
        target = image_format(requirements, settings)
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
        cache_key = self.derivatives.key(original, selection, "media.prepare.image", parameters)
        lock = self._conversion_locks.setdefault(cache_key, asyncio.Lock())
        async with self.execution.keyed_operation("media.prepare_image", lock):
            cached = await self.results.find_cached("media.prepare.image", cache_key)
            if cached is not None:
                track_completed(cached.ref.identifier)
                frame_info = cast(list[dict[str, JsonValue]], cached.parameters["frames"])
                derivatives = {
                    output.name: await self.artifacts.get(output.artifact)
                    for output in cached.outputs
                }
            else:
                await self.execution.blocking(
                    self.artifacts.content.temporary.mkdir, parents=True, exist_ok=True
                )
                workspace = await acquire_owned(
                    self.execution.blocking(
                        tempfile.TemporaryDirectory,
                        prefix="images-",
                        dir=self.artifacts.content.temporary,
                    ),
                    lambda value: self.execution.control_blocking(value.cleanup),
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
                            make_group(
                                f"S1:F{frame['frame']}",
                                original.display_name,
                                original,
                                SourceSelection.model_validate(frame["selection"]),
                            )
                            for frame in frame_info
                        ),
                        outputs=outputs,
                        parameters={
                            **parameters,
                            "cache_key": cache_key,
                            "frames": frame_info,
                            "representations": {
                                f"image-{index}": {
                                    "kind": "prepared",
                                    "sources": [
                                        ContentSelection(
                                            artifact=original.ref, selection=frame.selection
                                        ).model_dump(mode="json")
                                    ],
                                }
                                for index, frame in enumerate(frames)
                            },
                        },
                        coverage={"selection": selection.model_dump(mode="json")},
                        mappings=tuple(
                            OutputMapping(
                                output_name=f"image-{index}",
                                selection=SourceSelection(
                                    start_byte=0, end_byte=len(outputs[f"image-{index}"].data)
                                ),
                                relation="atomic",
                                origins=(
                                    ContentSelection(
                                        artifact=original.ref, selection=frame.selection
                                    ),
                                ),
                            )
                            for index, frame in enumerate(frames)
                            if outputs[f"image-{index}"].data
                        ),
                    )
                    track_completed(derived.ref.identifier)
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
                    group=make_group(
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
        groups = split_sources(
            sources, settings.video.max_frames_per_request, settings.max_inline_bytes
        )
        mode = "sampled" if int(cast(int, info["frames"])) > 1 else "image"
        batches = tuple(
            make_batch(
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

    async def pdf(
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
                group=make_group("S1", original.display_name, original, selection),
                metadata={"representation": "native_pdf"},
            )
            coverage: dict[str, JsonValue] = {"mode": "native_pdf", "pages": list(pages)}
            return MediaPlan(
                original,
                selection,
                (make_batch(original, (source,), selection, coverage),),
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
        async with self.execution.keyed_operation("media.prepare_pdf", lock):
            return await self._render_pdf(
                original,
                selection,
                settings,
                path,
                info,
                pages,
                visual,
                image_format(requirements, settings) if visual else "text/plain",
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
        cached = await self.derivatives.cached(
            "media.prepare.document",
            self.derivatives.key(original, selected, "media.prepare.document", parameters),
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
        derivatives = cached or await self.derivatives.derive(
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
        if cached is None:
            data = await self.artifacts.read_bytes(derivatives["document"].ref)
            if len(data) > settings.max_inline_bytes:
                raise NousError(
                    "media.input_too_large",
                    "The adopted native PDF exceeds the request byte allowance",
                    details={"bytes": len(data), "max_bytes": settings.max_inline_bytes},
                )
        source = PreparedSource(
            f"S1:P{pages[0]}-{pages[-1]}",
            original.display_name,
            media=(BinaryContent(data=data, media_type="application/pdf"),),
            group=make_group(
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
            original, selection, (make_batch(original, (source,), selected, coverage),), coverage
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
                key = self.derivatives.key(
                    original, page_selection, "media.prepare.pdf", parameters
                )
                cached = await self.derivatives.cached("media.prepare.pdf", key)
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
                    derivatives = await self.derivatives.derive(
                        original, page_selection, "media.prepare.pdf", content, parameters
                    )
                    cached_pages[number] = derivatives
                    page = PageData(
                        number,
                        await self.artifacts.read_text(
                            derivatives["text"].ref, max_bytes=settings.max_inline_bytes
                        ),
                        await self.artifacts.read_bytes(
                            derivatives["image"].ref, max_bytes=settings.max_inline_bytes
                        )
                        if visual
                        else None,
                    )
                if not page.text.strip() and not visual:
                    raise NousError(
                        "media.page_requires_vision",
                        "Selected PDF page has no extractable text",
                        details={"page": page.page, "artifact": original.ref.identifier},
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
                            slot_key="S1",
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
            for group in split_sources(
                sources, settings.pdf.max_pages_per_request, settings.max_inline_bytes
            ):
                actual = tuple(cast(int, source.metadata["page"]) for source in group)
                batches.append(
                    make_batch(
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
