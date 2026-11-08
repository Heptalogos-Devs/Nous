"""Immutable user content and the precise sources of processing results."""

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.common.errors import NousError
from nous.storage.content_types import BlobRef as BlobRef


def is_text_media_type(media_type: str) -> bool:
    """Identify formats whose contents are handled as UTF-8 text."""
    return media_type.startswith("text/") or media_type in {
        "application/json",
        "application/xml",
        "application/xhtml+xml",
    }


class ContentValue(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class ArtifactRef(ContentValue):
    id: str
    identifier: str
    instance_id: str


class ResultRef(ContentValue):
    id: str
    identifier: str
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
        groups = (
            bool(self.pages),
            self.start_line is not None or self.end_line is not None,
            self.start_byte is not None or self.end_byte is not None,
            any(value is not None for value in (self.start_ms, self.end_ms, self.frame, self.crop)),
        )
        if sum(groups) > 1:
            raise ValueError("A selection cannot mix page, text and media coordinates")
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

    @property
    def is_whole(self) -> bool:
        return not self.pages and all(
            value is None
            for value in (
                self.start_ms,
                self.end_ms,
                self.start_line,
                self.end_line,
                self.start_byte,
                self.end_byte,
                self.frame,
                self.crop,
            )
        )

    def restrict(self, selected: SourceSelection) -> SourceSelection:
        """Resolve absolute subcoordinates without extending the declared input range."""
        if self.is_whole:
            return selected
        if selected.is_whole:
            return self

        def family(value: SourceSelection) -> str:
            if value.pages:
                return "pages"
            if value.start_line is not None or value.end_line is not None:
                return "lines"
            if value.start_byte is not None or value.end_byte is not None:
                return "bytes"
            return "media"

        def invalid() -> NousError:
            return NousError(
                "content.invalid_selection",
                "Selection extends outside its declared input",
                details={
                    "input": self.model_dump(mode="json"),
                    "selected": selected.model_dump(mode="json"),
                },
            )

        kind = family(self)
        if family(selected) != kind:
            raise invalid()
        if kind == "pages":
            if not set(selected.pages).issubset(self.pages):
                raise invalid()
            return selected
        values = self.model_dump()
        values.update(selected.model_dump(exclude_none=True, exclude_defaults=True))
        for low, high in (
            ("start_line", "end_line"),
            ("start_byte", "end_byte"),
            ("start_ms", "end_ms"),
        ):
            lower, upper = getattr(self, low), getattr(self, high)
            chosen_lower, chosen_upper = values[low], values[high]
            if lower is not None and (chosen_lower is None or chosen_lower < lower):
                raise invalid()
            if upper is not None and (chosen_upper is None or chosen_upper > upper):
                raise invalid()
        if self.frame is not None and values["frame"] != self.frame:
            raise invalid()
        if self.crop is not None:
            left, top, right, bottom = self.crop
            crop = values["crop"]
            if crop is None or not (
                left <= crop[0] < crop[2] <= right and top <= crop[1] < crop[3] <= bottom
            ):
                raise invalid()
        return SourceSelection.model_validate(values)


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


class SelectedText(ContentValue):
    """Exact source bytes and their absolute coordinate origin."""

    content: ContentSelection
    blob: BlobRef
    data: bytes
    text: str
    start_byte: int
    end_byte: int
    first_line: int


class ContentRepresentation(ContentValue):
    kind: str
    result_ref: ResultRef
    operation: str
    output_name: str
    content: ContentSelection
    name: str
    media_type: str
    source_selections: tuple[ContentSelection, ...]
    coverage: dict[str, JsonValue]
    body_text: str | None = None


class ContentReadView(ContentValue):
    source: ContentSelection
    name: str
    media_type: str
    byte_length: int
    body_text: str | None = None
    available_representations: tuple[ContentRepresentation, ...] = ()
    export_ref: ArtifactRef
    coverage: dict[str, JsonValue]


class OriginSlice(ContentValue):
    origin: ContentSelection
    transform_ref: ResultRef | None = None


class InputPart(ContentValue):
    content: ContentSelection
    representation: str = "original"
    source_media_type: str | None = None
    media_type: str | None = None
    origins: tuple[OriginSlice, ...] = ()


SourceRole = Literal[
    "source", "reference", "current_basis", "inspiration", "revision_base", "edit_target", "context"
]


class SelectedSource(ContentValue):
    """One logical source slot and its ordered, independently selected regions."""

    slot_key: str = Field(min_length=1)
    role: SourceRole = "source"
    selections: tuple[ContentSelection, ...] = Field(min_length=1)
    display_name: str | None = None


class SourceFacts(ContentValue):
    """Detected facts for an immutable original, separate from its declared MIME."""

    artifact: ArtifactRef
    declared_mime: str
    detected_mime: str
    kind: Literal["text", "image", "pdf", "audio", "video"]
    details: dict[str, JsonValue] = Field(default_factory=dict)


class SelectedInputs(ContentValue):
    """The original material choices, independent of physical representations."""

    sources: tuple[SelectedSource, ...]
    facts: tuple[SourceFacts, ...] = ()

    def identity(self) -> list[dict[str, JsonValue]]:
        return [source.model_dump(mode="json") for source in self.sources]


class SourceContext(ContentValue):
    """Known original extents and actual selections delivered to a request."""

    kind: Literal["text", "image", "pdf", "audio", "video"]
    selections: tuple[SourceSelection, ...] = ()
    duration_ms: int | None = Field(default=None, ge=0)
    page_count: int | None = Field(default=None, ge=0)
    byte_length: int | None = Field(default=None, ge=0)
    frame_count: int | None = Field(default=None, ge=0)
    width: int | None = Field(default=None, ge=0)
    height: int | None = Field(default=None, ge=0)
    audio: Literal["included", "excluded", "absent", "processed"] | None = None


class InputGroup(ContentValue):
    """One local source key, with every independently addressable representation."""

    key: str
    slot_key: str = Field(min_length=1)
    name: str
    role: SourceRole = "source"
    parts: tuple[InputPart, ...] = Field(min_length=1)
    context: SourceContext | None = None


class TextInput(ContentValue):
    """A file, text or artifact and its exact selection for text operations."""

    kind: Literal["file", "text", "artifact"]
    path: str | None = None
    text: str | None = None
    artifact: ArtifactRef | str | None = None
    selection: SourceSelection = Field(default_factory=SourceSelection)

    @model_validator(mode="after")
    def matching_payload(self) -> TextInput:
        payload = {"file": self.path, "text": self.text, "artifact": self.artifact}
        if payload[self.kind] is None or any(
            value is not None for key, value in payload.items() if key != self.kind
        ):
            raise ValueError("An input must contain exactly its declared payload")
        return self


class InputSpec(TextInput):
    role: SourceRole = "source"
    slot_key: str | None = Field(default=None, min_length=1)
    display_name: str | None = None


class OutputContent(ContentValue):
    data: bytes
    media_type: str = "text/markdown"
    display_name: str = "result.md"


class ResultOutput(ContentValue):
    name: str
    artifact: ArtifactRef


class OutputMapping(ContentValue):
    """A producer-declared relation between one output range and its origins."""

    output_name: str
    selection: SourceSelection
    relation: Literal["identity", "atomic", "derived"]
    origins: tuple[ContentSelection, ...] = Field(min_length=1)


class ProcessingResult(ContentValue):
    ref: ResultRef
    operation: str
    inputs: tuple[InputGroup, ...]
    outputs: tuple[ResultOutput, ...]
    mappings: tuple[OutputMapping, ...] = ()
    dependencies: tuple[ArtifactRef | ResultRef, ...] = ()
    producer: dict[str, JsonValue]
    request: dict[str, JsonValue]
    parameters: dict[str, JsonValue]
    coverage: dict[str, JsonValue]
    formed_at: datetime
    recorded_at: datetime


MediaStrategy = Literal["description_only", "direct_structured", "describe_then_structure"]
