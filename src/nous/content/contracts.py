"""Immutable user content and the precise sources of processing results."""

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.storage.content_types import BlobRef as BlobRef


class ContentValue(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class ArtifactRef(ContentValue):
    id: str
    address: str
    instance_id: str


class ResultRef(ContentValue):
    id: str
    address: str
    instance_id: str


class SourceSelection(ContentValue):
    """Pages and lines are one-based inclusive; milliseconds are half-open."""

    pages: tuple[int, ...] = ()
    start_ms: int | None = Field(default=None, ge=0)
    end_ms: int | None = Field(default=None, gt=0)
    start_line: int | None = Field(default=None, ge=1)
    end_line: int | None = Field(default=None, ge=1)
    start_byte: int | None = Field(default=None, ge=0)
    end_byte: int | None = Field(default=None, ge=0)
    frame: int | None = Field(default=None, ge=0)
    crop: tuple[int, int, int, int] | None = None

    @model_validator(mode="after")
    def valid_range(self) -> SourceSelection:
        if any(page < 1 for page in self.pages):
            raise ValueError("PDF page numbers start at one")
        if self.end_ms is not None and self.end_ms <= (self.start_ms or 0):
            raise ValueError("The end time must follow the start time")
        if self.end_line is not None and self.end_line < (self.start_line or 1):
            raise ValueError("The end line must follow the start line")
        if self.end_byte is not None and self.end_byte < (self.start_byte or 0):
            raise ValueError("The end byte must follow the start byte")
        if (self.start_byte is not None or self.end_byte is not None) and (
            self.start_line is not None or self.end_line is not None
        ):
            raise ValueError("A selection uses either byte or line coordinates")
        if self.crop is not None:
            left, top, right, bottom = self.crop
            if min(left, top) < 0 or right <= left or bottom <= top:
                raise ValueError("Invalid pixel crop")
        return self


class Artifact(ContentValue):
    ref: ArtifactRef
    media_type: str
    display_name: str
    source: str | None = None
    received_at: datetime | None = None
    formed_at: datetime | None = None
    recorded_at: datetime
    producer_result_ref: ResultRef | None = None


class ContentSelection(ContentValue):
    """Coordinates refer to this artifact's own immutable body."""

    artifact: ArtifactRef
    selection: SourceSelection = Field(default_factory=SourceSelection)


class OriginSlice(ContentValue):
    origin: ContentSelection
    transform_ref: ResultRef | None = None


class InputPart(ContentValue):
    content: ContentSelection
    representation: str = "original"
    origins: tuple[OriginSlice, ...] = ()


class InputGroup(ContentValue):
    """One local source key, with every independently addressable representation."""

    key: str
    name: str
    parts: tuple[InputPart, ...] = Field(min_length=1)


class InputSpec(ContentValue):
    kind: Literal["file", "text", "artifact"]
    path: str | None = None
    text: str | None = None
    artifact: ArtifactRef | str | None = None
    selection: SourceSelection = Field(default_factory=SourceSelection)
    display_name: str | None = None

    @model_validator(mode="after")
    def matching_payload(self) -> InputSpec:
        payload = {"file": self.path, "text": self.text, "artifact": self.artifact}
        if payload[self.kind] is None or any(
            value is not None for key, value in payload.items() if key != self.kind
        ):
            raise ValueError("An input must contain exactly its declared payload")
        return self


class OutputContent(ContentValue):
    data: bytes
    media_type: str = "text/markdown"
    display_name: str = "result.md"


class ResultOutput(ContentValue):
    name: str
    artifact: ArtifactRef


class ProcessingResult(ContentValue):
    ref: ResultRef
    operation: str
    inputs: tuple[InputGroup, ...]
    outputs: tuple[ResultOutput, ...]
    producer: dict[str, JsonValue]
    request: dict[str, JsonValue]
    parameters: dict[str, JsonValue]
    coverage: dict[str, JsonValue]
    formed_at: datetime
    recorded_at: datetime


MediaStrategy = Literal["description_only", "direct_structured", "describe_then_structure"]
