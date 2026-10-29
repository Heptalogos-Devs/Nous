"""Extract only the authorized coordinates before supplying bytes to a model."""

import io
from pathlib import Path

import anyio
from nous_wave.contracts import SourceRegion, TimeSpan
from pypdf import PdfReader, PdfWriter

from nous.material.text import source_region_text
from nous.platform.content import ContentObject, ContentStore
from nous.platform.media import MediaService
from nous.platform.models.generation import MediaInput


def pdf_pages(path: Path, region: SourceRegion) -> bytes:
    reader = PdfReader(path)
    start, end = (region.page_start or 1) - 1, region.page_end or len(reader.pages)
    if not 0 <= start < end <= len(reader.pages):
        raise ValueError("Page selection is outside the retained PDF")
    writer = PdfWriter()
    for index in range(start, end):
        writer.add_page(reader.pages[index])
    output = io.BytesIO()
    writer.write(output)
    if output.tell() > 32 * 1024 * 1024:
        raise ValueError("Selected PDF pages exceed the 32 MiB model input limit")
    return output.getvalue()


async def selected_input(
    content: ContentStore,
    media: MediaService,
    source: ContentObject,
    reference: str,
    region: SourceRegion | None = None,
    *,
    inline_text: str | None = None,
) -> MediaInput:
    if region and (region.page_start is not None or region.page_end is not None):
        if source.media_type != "application/pdf":
            raise ValueError("Page selection requires a PDF")
        async with content.protect(source) as path:
            data = await anyio.to_thread.run_sync(pdf_pages, path, region)
        return MediaInput(data, "application/pdf", reference)
    textual = region is not None and any(
        v is not None
        for v in (region.text_start, region.text_end, region.json_pointer, region.line_number)
    )
    if inline_text is not None:
        text = source_region_text(inline_text, region) if textual and region else inline_text
        data = text.encode("utf-8")
        if len(data) > 32 * 1024 * 1024:
            raise ValueError("Selected text exceeds the 32 MiB model input limit")
        return MediaInput(data, "text/plain", reference)
    derived = None
    if region and region.pixel_rect:
        if source.media_type.startswith("image/"):
            derived = await media.crop_image(source, region.pixel_rect)
        elif source.media_type.startswith("video/"):
            derived = await media.crop_video(source, region.pixel_rect, region.time_span)
        else:
            raise ValueError("Pixel selection requires an image or video")
    elif region and region.time_span:
        derived = await media.extract_clip(
            source, region.time_span, audio_only=source.media_type.startswith("audio/")
        )
    selected = derived.lease if derived else source
    audio = None
    try:
        if selected.media_type.startswith("audio/") and selected.media_type not in {
            "audio/wav",
            "audio/x-wav",
            "audio/mp3",
            "audio/mpeg",
        }:
            info = await media.probe(selected)
            if not info.duration_ms:
                raise ValueError("Audio preparation requires a known positive duration")
            audio = await media.extract_clip(
                selected, TimeSpan(start_ms=0, end_ms=info.duration_ms), audio_only=True
            )
            selected = audio.lease
        limit = media.max_derivative_bytes if textual else 32 * 1024 * 1024
        if selected.byte_len > limit:
            raise ValueError("Selected input exceeds the instance media processing limit")
        async with content.protect(selected) as path:
            data = await anyio.to_thread.run_sync(path.read_bytes)
        if textual and region:
            data = source_region_text(data.decode("utf-8"), region).encode("utf-8")
        if len(data) > 32 * 1024 * 1024:
            raise ValueError("Selected model input exceeds 32 MiB")
        return MediaInput(data, "text/plain" if textual else selected.media_type, reference)
    finally:
        if audio:
            await content.release(audio.lease)
        if derived:
            await content.release(derived.lease)
