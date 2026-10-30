"""Select composited animation frames on the original frame-duration timeline."""

from __future__ import annotations

from pathlib import Path

from nous_sdk.contracts.media import PixelRect, TimeSpan
from PIL import Image, ImageOps


def decode_animation_frames(
    max_pixels: int,
    max_frames: int,
    source: Path,
    directory: Path,
    positions: tuple[int, ...],
    rect: PixelRect | None,
) -> None:
    with Image.open(source) as image:
        if image.width * image.height > max_pixels:
            raise ValueError("Decoded image exceeds the instance pixel limit")
        if int(getattr(image, "n_frames", 1)) > max_frames:
            raise ValueError("Image exceeds the instance animation frame limit")
        remaining = set(positions)
        start = 0
        for index in range(int(getattr(image, "n_frames", 1))):
            image.seek(index)
            duration = image.info.get("duration")
            if not isinstance(duration, (int, float)) or duration <= 0:
                raise ValueError("Animation frame has no positive recorded duration")
            end = start + int(duration)
            selected = {value for value in remaining if start <= value < end}
            if selected:
                # Pillow seek applies the format's blend and disposal semantics.
                frame = ImageOps.exif_transpose(image).convert("RGBA")
                if rect is not None:
                    frame = frame.crop((rect.x, rect.y, rect.x + rect.width, rect.y + rect.height))
                for value in selected:
                    frame.save(directory / f"{value}.png", format="PNG")
                remaining.difference_update(selected)
            start = end
            if not remaining:
                return
        raise ValueError("Animation did not cover every requested frame position")


def animation_timeline(
    source: Path, directory: Path, span: TimeSpan, rect: PixelRect | None
) -> Path:
    """FFmpeg concat input with the actual clipped duration of every composited frame."""
    lines = ["ffconcat version 1.0"]
    with Image.open(source) as image:
        start = 0
        ordinal = 0
        last_duration = 0
        for index in range(int(getattr(image, "n_frames", 1))):
            image.seek(index)
            duration = image.info.get("duration")
            if not isinstance(duration, (int, float)) or duration <= 0:
                raise ValueError("Animation frame has no positive recorded duration")
            end = start + int(duration)
            overlap = min(end, span.end_ms) - max(start, span.start_ms)
            if overlap > 0:
                last_duration = overlap
                frame = ImageOps.exif_transpose(image).convert("RGBA")
                if rect is not None:
                    frame = frame.crop((rect.x, rect.y, rect.x + rect.width, rect.y + rect.height))
                name = f"frame-{ordinal}.png"
                frame.save(directory / name, format="PNG")
                lines.extend(
                    (f"file '{name}'", "option framerate 1000", f"duration {overlap / 1000:.3f}")
                )
                ordinal += 1
            start = end
            if end >= span.end_ms:
                break
        if not ordinal or start < span.end_ms:
            raise ValueError("Animation does not cover the requested interval")
    # The image demuxer supplies 1 ms packets. Close the last hold at its
    # penultimate millisecond and repeat that same image for the terminal packet.
    # A packet at the exact end would be discarded by -t, shortening the hold.
    if last_duration > 1:
        lines[-1] = f"duration {(last_duration - 1) / 1000:.3f}"
        lines.extend((f"file 'frame-{ordinal - 1}.png'", "option framerate 1000"))
    manifest = directory / "frames.ffconcat"
    manifest.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return manifest
