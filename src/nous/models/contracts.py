"""Task input and output values shared by prompt, model and media owners."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from pydantic import BaseModel, ConfigDict, JsonValue
from pydantic_ai.messages import BinaryContent

if TYPE_CHECKING:
    from nous.content.contracts import ArtifactRef, SourceSelection


@dataclass(frozen=True)
class PreparedSource:
    key: str
    name: str
    text: str | None = None
    media: tuple[BinaryContent, ...] = ()
    artifact: ArtifactRef | None = None
    selection: SourceSelection | None = None
    metadata: dict[str, JsonValue] = field(default_factory=dict)


@dataclass(frozen=True)
class ReadingInput:
    question: str = ""
    instructions: str = ""
    sources: tuple[PreparedSource, ...] = ()


class InformationItem(BaseModel):
    model_config = ConfigDict(extra="forbid")
    text: str
    source_keys: list[str]


class ExtractedInformation(BaseModel):
    model_config = ConfigDict(extra="forbid")
    title: str
    items: list[InformationItem]


class PassageSelection(BaseModel):
    model_config = ConfigDict(extra="forbid")
    keys: list[str]


OUTPUT_TYPES: dict[str, type[BaseModel] | None] = {
    "reading-summary": None,
    "media-description": None,
    "information-extraction": ExtractedInformation,
    "passage-selection": PassageSelection,
    "custom-reading": None,
    "transcription": None,
}


@dataclass(frozen=True)
class GenerationResult:
    output: JsonValue
    producer: dict[str, JsonValue]
    usage: dict[str, JsonValue]
    request: dict[str, JsonValue]
    attempts: int
    finish_reason: str | None = None
    response: dict[str, JsonValue] = field(default_factory=dict)


@dataclass(frozen=True)
class EmbeddingResult:
    vectors: tuple[tuple[float, ...], ...]
    raw_vectors: tuple[tuple[float, ...], ...]
    producer: dict[str, JsonValue]
    usage: dict[str, JsonValue]
    request: dict[str, JsonValue]
    attempts: int
    npy: bytes

    @property
    def dimensions(self) -> int:
        return len(self.vectors[0]) if self.vectors else 0
