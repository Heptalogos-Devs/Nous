"""Generation requests, successful computations and recovery values."""

from __future__ import annotations

import copy
import hashlib
import json
from dataclasses import dataclass
from typing import Literal

from pydantic import JsonValue

from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    InputGroup,
    OutputContent,
    OutputMapping,
    ProcessingResult,
    ResultRef,
    SelectedInputs,
)
from nous.content.results import PreparedOutputs

from ..contracts import GenerationResult, ReadingInput
from ..input import InputPlan, RequestGroup
from ..requests import FrozenTask
from .computation import FrozenLanguageTask


def schema_request_value(schema: dict[str, JsonValue] | None) -> str | None:
    """Represent the complete schema in the order supplied to the model."""
    if schema is None:
        return None
    return json.dumps(schema, ensure_ascii=False, separators=(",", ":"), allow_nan=False)


@dataclass(frozen=True)
class PreparedRun:
    artifacts: tuple[Artifact, ...]
    batches: tuple[RequestGroup, ...]
    coverage: dict[str, JsonValue]
    frozen: FrozenTask
    question: str = ""
    instructions: str = ""
    schema: dict[str, JsonValue] | None = None
    preparations: tuple[str, ...] = ()
    input_plan: InputPlan | None = None


@dataclass(frozen=True)
class GenerationOutcome:
    artifacts: tuple[Artifact, ...]
    results: tuple[ProcessingResult, ...]
    coverage: dict[str, JsonValue]
    preparations: tuple[str, ...] = ()


@dataclass(frozen=True)
class ProducedTask:
    """A successful language computation whose named outputs can be adopted without another call."""

    operation: str
    producer: dict[str, JsonValue]
    actual_inputs: tuple[InputGroup, ...]
    outputs: dict[str, OutputContent]
    request: dict[str, JsonValue]
    parameters: dict[str, JsonValue]
    coverage: dict[str, JsonValue]
    completed_stage_refs: tuple[str, ...]
    selected_inputs: SelectedInputs | None = None
    dependencies: tuple[ArtifactRef | ResultRef, ...] = ()


@dataclass(frozen=True)
class GenerationRequest:
    task: str | FrozenLanguageTask
    inputs: SelectedInputs
    question: str = ""
    instructions: str = ""
    schema: dict[str, JsonValue] | None = None
    mode: Literal["joint", "independent_batches"] = "joint"
    dependencies: tuple[ArtifactRef | ResultRef, ...] = ()
    document_mode: str = "auto"
    without_audio: bool = False

    def capture(self) -> GenerationRequest:
        """Detach the request from caller-owned values before an operation waits."""
        return copy.deepcopy(self)

    def identity(self) -> str:
        return hashlib.sha256(
            json.dumps(
                {
                    "task": self.task
                    if isinstance(self.task, str)
                    else self.task.model_dump(mode="json"),
                    "inputs": self.inputs.identity(),
                    "question": self.question,
                    "instructions": self.instructions,
                    "schema": schema_request_value(self.schema),
                    "mode": self.mode,
                    "dependencies": [ref.model_dump(mode="json") for ref in self.dependencies],
                    "document_mode": self.document_mode,
                    "without_audio": self.without_audio,
                },
                sort_keys=True,
                ensure_ascii=False,
            ).encode()
        ).hexdigest()


@dataclass(frozen=True)
class PreparedProduction:
    """Published outputs and their adoption facts, reusable after a transaction rollback."""

    operation: str
    producer: dict[str, JsonValue]
    actual_inputs: tuple[InputGroup, ...]
    outputs: PreparedOutputs
    request: dict[str, JsonValue]
    parameters: dict[str, JsonValue]
    coverage: dict[str, JsonValue]
    producer_key: str | None
    completed_stage_refs: tuple[str, ...]
    dependencies: tuple[ArtifactRef | ResultRef, ...]
    mappings: tuple[OutputMapping, ...]


@dataclass(frozen=True)
class PendingResponse:
    """A successful response and any published bytes awaiting adoption."""

    generated: GenerationResult
    outputs: PreparedOutputs | None
    reading: ReadingInput
    inputs: tuple[InputGroup, ...]
    identity: str
    producer_key: str
    frozen_task_ref: ArtifactRef | None
    selected_inputs: SelectedInputs | None
    dependencies: tuple[ArtifactRef | ResultRef, ...]
    interpretation_contract: dict[str, JsonValue]
    requested_inputs: tuple[InputGroup, ...] | None = None
    requested_coverage: dict[str, JsonValue] | None = None


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
