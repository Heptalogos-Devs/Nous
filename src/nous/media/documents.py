"""Pure PDFium/Pillow work: all native handles remain inside the actual worker."""

import io
import threading
from contextlib import closing
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol, cast

import PIL
from PIL import Image, ImageOps
from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import SourceSelection

# PDFium calls across all documents share this process lock, held by the actual worker.
_PDF_LOCK = threading.Lock()


class _Bitmap(Protocol):
    def to_pil(self) -> Image.Image: ...
    def close(self) -> None: ...


class _Renderable(Protocol):
    def render(self, *, scale: float) -> _Bitmap: ...


@dataclass(frozen=True)
class PageData:
    page: int
    text: str
    image: bytes | None


@dataclass(frozen=True)
class ImageData:
    data: bytes
    media_type: str
    selection: SourceSelection
    metadata: dict[str, JsonValue]


def inspect_image(path: Path) -> dict[str, JsonValue]:
    with Image.open(path) as image:
        frames = getattr(image, "n_frames", 1)
        duration = 0
        for index in range(frames):
            image.seek(index)
            duration += int(image.info.get("duration", 0))
        return {
            "kind": "image",
            "format": image.format,
            "width": image.width,
            "height": image.height,
            "frames": frames,
            "duration_ms": duration,
            "mode": image.mode,
            "orientation": int(image.getexif().get(274, 1)),
            "media_type": Image.MIME.get(image.format or "", "application/octet-stream"),
            "tool": "Pillow",
            "version": PIL.__version__,
        }


def validate_image_selection(selection: SourceSelection, frames: int) -> None:
    """Validate coordinates before choosing direct bytes or normalized image frames."""
    unsupported = [
        field
        for field, present in (
            ("pages", bool(selection.pages)),
            ("start_line", selection.start_line is not None),
            ("end_line", selection.end_line is not None),
        )
        if present
    ]
    if unsupported:
        raise NousError(
            "media.invalid_selection",
            "Image selections do not support document pages or text lines",
            details={"fields": unsupported},
        )
    if selection.frame is not None and not 0 <= selection.frame < frames:
        raise NousError(
            "media.invalid_selection",
            "Selected image frame is outside the image",
            details={"frame": selection.frame, "frames": frames},
        )
    if frames == 1 and (selection.start_ms is not None or selection.end_ms is not None):
        raise NousError(
            "media.invalid_selection",
            "A static image does not have a selectable time interval",
        )


def prepare_images(
    path: Path, selection: SourceSelection, max_side: int, interval_ms: int
) -> tuple[ImageData, ...]:
    """Sample animations by frame durations and return oriented, cropped image bytes."""
    with Image.open(path) as image:
        frames = getattr(image, "n_frames", 1)
        validate_image_selection(selection, frames)
        chosen: list[tuple[int, int, int]] = []
        position, next_sample = 0, selection.start_ms or 0
        for index in range(frames):
            image.seek(index)
            duration = int(image.info.get("duration", 100)) if frames > 1 else 0
            if selection.frame is not None:
                intersects_time = position + max(duration, 1) > (selection.start_ms or 0) and (
                    selection.end_ms is None or position < selection.end_ms
                )
                if index == selection.frame and intersects_time:
                    chosen.append((index, position, duration))
            elif frames == 1 or (
                position + max(duration, 1) > next_sample
                and (selection.end_ms is None or position < selection.end_ms)
            ):
                chosen.append((index, position, duration))
                next_sample = max(next_sample + interval_ms, position + duration)
            position += duration
        if not chosen:
            raise NousError("media.invalid_selection", "Selected image frame/time is unavailable")
        outputs: list[ImageData] = []
        for index, position, duration in chosen:
            image.seek(index)
            oriented = ImageOps.exif_transpose(image)
            original_size = oriented.size
            if selection.crop is not None:
                if selection.crop[2] > oriented.width or selection.crop[3] > oriented.height:
                    raise NousError("media.invalid_selection", "Pixel crop exceeds the image")
                oriented = oriented.crop(selection.crop)
            oriented.thumbnail((max_side, max_side), Image.Resampling.LANCZOS)
            normalized = oriented.convert("RGBA" if "A" in oriented.getbands() else "RGB")
            output = io.BytesIO()
            normalized.save(output, "PNG")
            selected = selection
            if frames > 1:
                updates: dict[str, object] = {"frame": index}
                if selection.start_ms is not None or selection.end_ms is not None:
                    updates["start_ms"] = max(position, selection.start_ms or 0)
                    updates["end_ms"] = min(
                        position + max(duration, 1),
                        selection.end_ms
                        if selection.end_ms is not None
                        else position + max(duration, 1),
                    )
                selected = selection.model_copy(update=updates)
            outputs.append(
                ImageData(
                    output.getvalue(),
                    "image/png",
                    selected,
                    {
                        "frame": index,
                        "actual_time_ms": position,
                        "duration_ms": duration,
                        "source_width": original_size[0],
                        "source_height": original_size[1],
                        "width": normalized.width,
                        "height": normalized.height,
                        "exif_orientation_applied": True,
                        "tool": "Pillow",
                        "version": PIL.__version__,
                    },
                )
            )
        return tuple(outputs)


def inspect_pdf(path: Path) -> dict[str, JsonValue]:
    import pypdfium2 as pdfium

    try:
        with _PDF_LOCK, pdfium.PdfDocument(path) as document:
            return {
                "kind": "pdf",
                "pages": len(document),
                "tool": "PDFium",
                "version": str(pdfium.PYPDFIUM_INFO),
            }
    except pdfium.PdfiumError as exc:
        raise NousError("media.invalid_pdf", "PDF document cannot be opened or read") from exc


def prepare_pdf_pages(
    path: Path, pages: tuple[int, ...], dpi: int, visual: bool
) -> tuple[PageData, ...]:
    """Read one-based pages, closing native handles before returning text and image bytes."""
    import pypdfium2 as pdfium

    try:
        with _PDF_LOCK, pdfium.PdfDocument(path) as document:
            outputs: list[PageData] = []
            for number in pages:
                if not 1 <= number <= len(document):
                    raise NousError(
                        "media.invalid_selection",
                        "PDF page selection is outside the document",
                        details={"page": number, "pages": len(document)},
                    )
                with closing(document[number - 1]) as page:
                    with closing(page.get_textpage()) as textpage:
                        text = textpage.get_text_range()
                    image: bytes | None = None
                    if visual:
                        with closing(cast(_Renderable, page).render(scale=dpi / 72)) as bitmap:
                            pil = bitmap.to_pil()
                            try:
                                output = io.BytesIO()
                                pil.save(output, "PNG")
                                image = output.getvalue()
                            finally:
                                pil.close()
                    outputs.append(PageData(number, text, image))
            return tuple(outputs)
    except pdfium.PdfiumError as exc:
        raise NousError("media.invalid_pdf", "PDF document cannot be opened or read") from exc
