"""Generation requests, successful computations and recovery values."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Annotated, Literal

from pydantic import Field, JsonValue

from nous.content.contracts import (
    Artifact,
    ContentValue,
    ExternalInput,
    OutputContent,
    ResultFacts,
    ResultRef,
    SelectedInputs,
    SourceTarget,
)

from ..contracts import ReadingInput
from ..input import InputPlan
from ..requests import FrozenTask


class PromptText(ContentValue):
    type: Literal["text"]
    text: str


class PromptMedia(ContentValue):
    type: Literal["media"]
    media_type: str
    bytes: int = Field(ge=0)
    source_key: str
    metadata: dict[str, JsonValue]


type PromptPart = Annotated[PromptText | PromptMedia, Field(discriminator="type")]


class PromptMessage(ContentValue):
    role: Literal["system", "user", "assistant"]
    content: tuple[PromptPart, ...]


class GenerationRequestFacts(ContentValue):
    """The actual model-visible task, prompt, schema and selected route."""

    task: str
    contract_id: str
    prompt_id: str
    question: str
    instructions: str
    messages: tuple[PromptMessage, ...]
    output_schema: dict[str, JsonValue] | None = Field(default=None, alias="schema")
    model_id: str
    requested_model_name: str
    parameters: dict[str, JsonValue]
    output_mode: str
    max_output_tokens: int = Field(gt=0)
    attempt_reports: tuple[dict[str, JsonValue], ...] = ()
    structured_repairs: int = Field(default=0, ge=0)
    repair_feedbacks: tuple[dict[str, JsonValue], ...] = ()


class TranscriptionRequestFacts(ContentValue):
    """The actual transcription request; audio bytes remain in the input artifact."""

    task: str
    filename: str
    bytes: int = Field(ge=0)
    language: str | None = None
    hint: str | None = None
    attempt_reports: tuple[dict[str, JsonValue], ...] = ()


@dataclass(frozen=True)
class PreparedRun:
    artifacts: tuple[Artifact, ...]
    frozen: FrozenTask
    input_plan: InputPlan
    preparations: tuple[ResultRef, ...] = ()


@dataclass(frozen=True)
class GenerationRequest:
    task: str
    inputs: SelectedInputs
    question: str = ""
    instructions: str = ""
    output_schema: dict[str, JsonValue] | None = None
    input_mode: Literal["joint", "independent"] = "joint"
    document_mode: Literal["auto", "text", "visual"] = "auto"
    without_audio: bool = False
    stream: bool = False


@dataclass(frozen=True)
class CompletedTask:
    facts: ResultFacts
    outputs: Mapping[str, OutputContent]


class RunModel(ContentValue):
    task: str
    sources: tuple[SourceTarget, ...] = Field(min_length=1)
    question: str = ""
    instructions: str = ""
    output_schema: dict[str, JsonValue] | ExternalInput | None = None
    input_mode: Literal["joint", "independent"] = "joint"
    document_mode: Literal["auto", "text", "visual"] = "auto"
    without_audio: bool = False
    stream: bool = False


class PreviewPrompt(ContentValue):
    prompt: str
    sources: tuple[SourceTarget, ...] = ()
    question: str = ""
    instructions: str = ""
    output_schema: dict[str, JsonValue] | ExternalInput | None = None
    task: str | None = None
    input_mode: Literal["joint", "independent"] = "joint"


class ProbeModel(ContentValue):
    model: str
    source: SourceTarget | None = None


class DiscoverModels(ContentValue):
    provider: str


class ModelOutcome(ContentValue):
    results: tuple[ResultRef, ...]
    coverage: dict[str, JsonValue]


@dataclass(frozen=True)
class PendingGeneration:
    frozen: FrozenTask
    reading: ReadingInput
    coverage: dict[str, JsonValue]
    formats: tuple[str, ...]
    transcription_task: str | None

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            "status": "requires_transcription",
            "task": self.frozen.task,
            "model_id": self.frozen.routes[0].model_id,
            "formats": list(self.formats),
            "transcription_task": self.transcription_task,
            "source_groups": [
                source.group.model_dump(mode="json")
                for source in self.reading.sources
                if source.group
            ],
            "coverage": self.coverage,
            "deferred_routes": [route.model_id for route in self.frozen.routes[1:]],
        }
