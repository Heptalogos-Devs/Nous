"""Exact media derivations preserve actual ordered inputs and producer identity."""

from typing import Annotated, Literal

from pydantic import Field, JsonValue, model_validator

from .common import Contract, Operation, ReferenceAddresses, TimePoint
from .media import MediaInfo, MediaReference, PixelRect, SourceRegion, TimeSpan

Digest = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
RepresentationKind = Literal[
    "extracted_text",
    "ocr",
    "transcript",
    "audio_description",
    "image_description",
    "scene_description",
    "summary",
    "structured_interpretation",
    "resource_synopsis",
    "sampled_frame",
    "audio_track",
    "crop",
    "clip",
    "other",
]
DerivationStrategy = Literal[
    "description_only", "direct_structured", "describe_then_structure", "preprocess"
]


class MediaTransformation(Contract):
    action: Literal["crop", "clip", "audio_track", "sample_frames"]
    pixel_rect: PixelRect | None = None
    time_span: TimeSpan | None = None
    sample_at_ms: tuple[int, ...] = Field(default=(), max_length=256)

    @model_validator(mode="after")
    def explicit_selection(self) -> "MediaTransformation":
        if self.action == "crop" and self.pixel_rect is None:
            raise ValueError("Cropping requires an exact pixel rectangle")
        if self.action == "clip" and self.time_span is None:
            raise ValueError("Clipping requires an exact time span")
        if self.action == "sample_frames":
            if not self.sample_at_ms or any(p < 0 for p in self.sample_at_ms):
                raise ValueError("Frame sampling requires nonnegative sample positions")
            if len(set(self.sample_at_ms)) != len(self.sample_at_ms):
                raise ValueError("Frame sampling positions must be distinct")
            if self.time_span is not None:
                raise ValueError("Frame positions are relative to the selected source")
        elif self.sample_at_ms:
            raise ValueError("Sample positions require frame sampling")
        if self.action in {"clip", "audio_track"} and self.pixel_rect is not None:
            raise ValueError("Audio extraction and clipping do not take a pixel rectangle")
        return self


class DeriveMedia(Operation):
    source: MediaReference
    transformation: MediaTransformation


class RepresentationRef(Contract):
    id: str = Field(min_length=1)
    revision: int = Field(ge=1)

    def media(self) -> MediaReference:
        return MediaReference(kind="representation", id=f"{self.id}@{self.revision}")


class DerivationInput(Contract):
    ordinal: int = Field(ge=0, le=63)
    source: MediaReference
    role: Literal[
        "source", "context", "description", "transcript", "extracted_text", "sampled_frame"
    ] = "source"


class ChannelAccess(Contract):
    visual: Literal["original", "representation", "unavailable"] = "unavailable"
    audio: Literal["original", "representation", "unavailable"] = "unavailable"
    source_text: Literal["original", "representation", "unavailable"] = "unavailable"


class DerivationProducer(Contract):
    signature: Digest
    implementation: str = Field(min_length=1, max_length=256)
    model_call_id: str | None = None
    prompt_digest: Digest | None = None
    schema_digest: Digest | None = None
    config_digest: Digest
    preprocessing_digest: Digest | None = None


class DerivationQuality(Contract):
    truncated: bool = False
    limitations: tuple[str, ...] = ()
    coverage: tuple[TimeSpan, ...] = ()


class TextRepresentation(Contract):
    kind: Literal["text"] = "text"
    text: str = Field(min_length=1, max_length=1_000_000)


class StructuredRepresentation(Contract):
    kind: Literal["structured"] = "structured"
    schema_id: str = Field(min_length=1)
    value: dict[str, JsonValue]


class ArtifactRepresentation(Contract):
    kind: Literal["artifact"] = "artifact"
    material_id: str
    content_hash: Digest
    byte_len: int = Field(ge=0)
    media_type: str
    media_info: MediaInfo


RepresentationPayload = Annotated[
    TextRepresentation | StructuredRepresentation | ArtifactRepresentation,
    Field(discriminator="kind"),
]


class CreateRepresentation(Operation):
    id: str | None = None
    expected_revision: int = Field(default=0, ge=0)
    inputs: tuple[DerivationInput, ...] = Field(min_length=1, max_length=64)
    kind: RepresentationKind
    strategy: DerivationStrategy = "description_only"
    payload: RepresentationPayload
    producer: DerivationProducer
    quality: DerivationQuality = Field(default_factory=DerivationQuality)
    timeline: MediaReference | None = None
    transformation: MediaTransformation | None = None

    @model_validator(mode="after")
    def ordered_inputs(self) -> "CreateRepresentation":
        if tuple(item.ordinal for item in self.inputs) != tuple(range(len(self.inputs))):
            raise ValueError("Derivation inputs require contiguous ordinals in their actual order")
        if len({item.source.exact() for item in self.inputs}) != len(self.inputs):
            raise ValueError("Derivation inputs cannot repeat a source")
        if self.expected_revision and self.id is None:
            raise ValueError("A representation revision requires its existing identity")
        if self.kind == "structured_interpretation":
            if (
                not isinstance(self.payload, StructuredRepresentation)
                or not self.producer.schema_digest
            ):
                raise ValueError(
                    "Structured interpretation requires a schema-bound structured payload"
                )
            if self.strategy == "description_only":
                raise ValueError(
                    "Description-only derivation cannot claim structured interpretation"
                )
        elif isinstance(self.payload, StructuredRepresentation):
            raise ValueError("Structured payload requires structured_interpretation kind")
        return self


class DerivedRepresentation(Contract):
    reference: RepresentationRef
    addresses: ReferenceAddresses | None = None
    namespace_id: str
    subject_id: str
    inputs: tuple[DerivationInput, ...]
    kind: RepresentationKind
    strategy: DerivationStrategy
    payload: RepresentationPayload | None
    producer: DerivationProducer
    access: ChannelAccess
    quality: DerivationQuality
    source_roots: tuple[str, ...]
    material_ids: tuple[str, ...]
    source_regions: dict[str, tuple[str, ...]] = Field(default_factory=dict)
    timeline: MediaReference | None = None
    transformation: MediaTransformation | None = None
    formed_at: TimePoint
    recorded_at: str
    commit_seq: int
    previous: RepresentationRef | None = None
    purged: bool = False


class DerivedSelector(Contract):
    kind: Literal["text_span", "json_pointer", "media"]
    start: int | None = Field(default=None, ge=0)
    end: int | None = Field(default=None, ge=0)
    json_pointer: str | None = None
    pixel_rect: PixelRect | None = None
    time_span: TimeSpan | None = None
    page_start: int | None = Field(default=None, ge=1)
    page_end: int | None = Field(default=None, ge=1)

    @model_validator(mode="after")
    def selected(self) -> "DerivedSelector":
        if self.kind == "text_span":
            if self.start is None or self.end is None or self.start >= self.end:
                raise ValueError("Text selection requires start < end")
            if any(
                v is not None
                for v in (
                    self.json_pointer,
                    self.pixel_rect,
                    self.time_span,
                    self.page_start,
                    self.page_end,
                )
            ):
                raise ValueError("Text selection cannot contain other coordinates")
        elif self.kind == "json_pointer":
            if self.json_pointer is None or (
                self.json_pointer and not self.json_pointer.startswith("/")
            ):
                raise ValueError("JSON selection requires an RFC 6901 pointer")
            if any(
                v is not None
                for v in (
                    self.start,
                    self.end,
                    self.pixel_rect,
                    self.time_span,
                    self.page_start,
                    self.page_end,
                )
            ):
                raise ValueError("JSON selection cannot contain other coordinates")
        elif (
            self.start is not None
            or self.end is not None
            or self.json_pointer is not None
            or not any(
                v is not None
                for v in (self.pixel_rect, self.time_span, self.page_start, self.page_end)
            )
        ):
            raise ValueError("Media selection requires only media coordinates")
        if self.page_start and self.page_end and self.page_start > self.page_end:
            raise ValueError("Page selection must be ordered")
        return self


class CreateDerivedRegion(Operation):
    id: str | None = None
    representation: RepresentationRef
    selector: DerivedSelector
    parent_id: str | None = None


class DerivedRegion(Contract):
    id: str
    addresses: ReferenceAddresses | None = None
    namespace_id: str
    subject_id: str
    representation: RepresentationRef
    selector: DerivedSelector | None
    parent_id: str | None = None
    recorded_at: str
    commit_seq: int
    purged: bool = False


class MediaLineage(Contract):
    source: MediaReference
    root_material_ids: tuple[str, ...]
    representations: tuple[DerivedRepresentation, ...]
    derived_regions: tuple[DerivedRegion, ...]
    source_regions: tuple[SourceRegion, ...] = ()
    unavailable_inputs: tuple[MediaReference, ...] = ()
    watermark: int


class DerivedBundle(Contract):
    representations: tuple[DerivedRepresentation, ...] = ()
    regions: tuple[DerivedRegion, ...] = ()

    @property
    def material_ids(self) -> tuple[str, ...]:
        return tuple(
            sorted(
                {identity for record in self.representations for identity in record.material_ids} | set(self.artifact_material_ids)
            )
        )

    @property
    def artifact_material_ids(self) -> tuple[str, ...]:
        return tuple(sorted({record.payload.material_id for record in self.representations if isinstance(record.payload, ArtifactRepresentation)}))

    @property
    def observation_ids(self) -> tuple[str, ...]:
        return tuple(
            sorted(
                {identity for record in self.representations for identity in record.source_roots}
            )
        )

    @property
    def source_region_ids(self) -> tuple[str, ...]:
        return tuple(
            sorted(
                {
                    identity
                    for record in self.representations
                    for ids in record.source_regions.values()
                    for identity in ids
                }
            )
        )
