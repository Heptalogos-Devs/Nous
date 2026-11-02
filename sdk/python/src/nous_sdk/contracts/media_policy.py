"""Instance media tools and resource limits, frozen for each preparation operation."""

from pydantic import Field

from .common import Contract


class MediaPolicy(Contract):
    ffmpeg: str = Field(default="ffmpeg", min_length=1)
    ffprobe: str = Field(default="ffprobe", min_length=1)
    max_pixels: int = Field(default=40_000_000, gt=0)
    max_frames: int = Field(default=256, gt=0)
    max_derivative_bytes: int = Field(default=128 * 1024 * 1024, gt=0)
    process_concurrency: int = Field(default=2, ge=1, le=64)
    probe_timeout_seconds: float = Field(default=30, gt=0)
    transform_timeout_seconds: float = Field(default=120, gt=0)
