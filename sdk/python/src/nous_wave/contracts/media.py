"""Original media and exact source coordinates shared by SDK and server."""

from typing import Literal

from pydantic import Field, model_validator

from .common import CognitiveTimes, Contract, Operation, ReferenceAddresses, TimePoint


class MediaInfo(Contract):
    kind: Literal["text", "image", "audio", "video", "document", "binary"]
    media_type: str
    width: int | None = None
    height: int | None = None
    duration_ms: int | None = None
    frames: int | None = None
    sample_rate: int | None = None
    channels: int | None = None


class PixelRect(Contract):
    x: int = Field(ge=0)
    y: int = Field(ge=0)
    width: int = Field(gt=0)
    height: int = Field(gt=0)


class TimeSpan(Contract):
    start_ms: int = Field(ge=0)
    end_ms: int = Field(gt=0)

    @model_validator(mode="after")
    def ordered(self) -> "TimeSpan":
        if self.end_ms <= self.start_ms:
            raise ValueError("media span must end after its start")
        return self


class MediaReference(Contract):
    kind: Literal["material", "source_region", "representation", "derived_region"]
    id: str
    observation_id: str | None = None

    def exact(self) -> str:
        return f"{self.kind.replace('_', '-')}:{self.id}"


class SourceRegion(Contract):
    id: str
    addresses: ReferenceAddresses | None = None
    material_id: str
    pixel_rect: PixelRect | None = None
    time_span: TimeSpan | None = None
    page_start: int | None = Field(default=None, ge=1)
    page_end: int | None = Field(default=None, ge=1)
    text_start: int | None = Field(default=None, ge=0)
    text_end: int | None = Field(default=None, ge=0)
    json_pointer: str | None = None
    line_number: int | None = Field(default=None, ge=1)


class BlobDocument(Contract):
    filename: str
    media_type: str
    source_type: str = "media"
    origin: dict[str, str] = Field(default_factory=dict)
    event_time: TimePoint | None = None
    observed_at: TimePoint | None = None


class AdoptMedia(Operation):
    material_id: str
    regions: tuple[SourceRegion, ...] = ()
    strategy: Literal["native", "description", "description_then_native"] = "native"
    instruction: str = "Describe and remember the supplied material."


class MediaEmbeddingInput(Contract):
    reference: MediaReference
    content_hash: str
    byte_len: int = Field(ge=0)
    media_type: str
    filename: str


class MediaRecord(Contract):
    reference: MediaReference
    addresses: ReferenceAddresses | None = None
    namespace_id: str
    subject_id: str
    material_id: str
    content_hash: str
    media_type: str
    byte_len: int
    filename: str
    text: str
    source_roots: tuple[str, ...]
    support_roots: tuple[str, ...]
    source_regions: dict[str, tuple[str, ...]] = Field(default_factory=dict)
    times: CognitiveTimes
    region: SourceRegion | None = None
    commit_seq: int
    deleted: bool = False
