"""Task input and output values shared by prompt, model and media owners."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from pydantic import JsonValue

if TYPE_CHECKING:
    from nous.media.contracts import PreparedSource


@dataclass(frozen=True)
class ReadingInput:
    question: str = ""
    instructions: str = ""
    sources: tuple[PreparedSource, ...] = ()


@dataclass(frozen=True)
class GenerationResult:
    output: JsonValue
    producer: dict[str, JsonValue]
    usage: dict[str, JsonValue]
    request: dict[str, JsonValue]
    attempts: int
    finish_reason: str | None = None
    response: dict[str, JsonValue] = field(default_factory=dict)
    reading: ReadingInput | None = None
    coverage: dict[str, JsonValue] = field(default_factory=dict)


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
