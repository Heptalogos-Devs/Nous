from pydantic import Field

from nous.configuration.metadata import ConfigModel, setting


class ImageConfig(ConfigModel):
    max_side: int = setting(2048, gt=0, description="Maximum normalized image side in pixels.")
    formats: list[str] = setting(
        ["image/png", "image/jpeg", "image/webp"],
        description="Preferred supported image encodings, in order.",
    )
    quality: int = setting(90, ge=1, le=100, description="JPEG and WebP encoder quality.")
    jpeg_background: str | None = setting(
        None, description="Explicit Pillow color for compositing transparent images to JPEG."
    )


class PDFConfig(ConfigModel):
    render_dpi: int = setting(144, gt=0, description="PDF page render resolution in DPI.")
    max_pages_per_request: int = setting(12, gt=0, description="Maximum PDF pages in one batch.")


class VideoConfig(ConfigModel):
    sample_interval_ms: int = setting(
        2000, gt=0, description="Target sample interval in milliseconds."
    )
    max_frames_per_request: int = setting(
        12, gt=0, description="Maximum video frames in one batch."
    )


class AudioConfig(ConfigModel):
    formats: list[str] = setting(
        ["audio/wav", "audio/mpeg", "audio/flac"],
        description="Preferred supported audio encodings, in order.",
    )
    max_seconds_per_request: float = setting(
        60, gt=0, description="Maximum continuous audio segment duration in seconds."
    )
    transcription_task: str | None = setting(
        None, description="Configured task for the standard audio transcription endpoint."
    )


class AnalysisConfig(ConfigModel):
    description_task: str = setting("describe-media", description="Task for media description.")
    extraction_task: str = setting(
        "extract-information", description="Task for structured extraction."
    )


class MediaConfig(ConfigModel):
    ffmpeg_path: str | None = setting(
        None, apply="restart", description="FFmpeg executable; null searches PATH."
    )
    ffprobe_path: str | None = setting(
        None, apply="restart", description="FFprobe executable; null searches PATH."
    )
    max_inline_bytes: int = setting(
        33554432, gt=0, description="Maximum materialized media bytes per model request."
    )
    image: ImageConfig = Field(default_factory=ImageConfig)
    pdf: PDFConfig = Field(default_factory=PDFConfig)
    video: VideoConfig = Field(default_factory=VideoConfig)
    audio: AudioConfig = Field(default_factory=AudioConfig)
    analysis: AnalysisConfig = Field(default_factory=AnalysisConfig)
