"""Frozen model task and per-route request values."""

import json
import math
from dataclasses import dataclass, field
from typing import cast

from pydantic import JsonValue
from pydantic_ai.messages import ModelMessage

from nous.configuration import ConfigSnapshot
from nous.content.contracts import ArtifactRef, ResultRef
from nous.media.configuration import MediaConfig
from nous.models.configuration import ModelExecutionConfig, ModelRoute, TaskAssignment

from .prompts import CompiledPrompt, PromptResource


@dataclass(frozen=True)
class UnavailableTask:
    """A frozen prerequisite failure, reported only if its route is selected."""

    task: str
    code: str
    message: str
    details: dict[str, JsonValue]
    retryable: bool


@dataclass(frozen=True)
class FrozenTask:
    task: str
    assignment: TaskAssignment
    routes: tuple[ModelRoute, ...]
    resource: PromptResource | None
    ineligible_routes: dict[str, JsonValue] = field(default_factory=dict)
    execution_limits: ModelExecutionConfig | None = None
    media_settings: MediaConfig | None = None
    transcription: ArtifactRef | UnavailableTask | None = None


@dataclass(frozen=True)
class PreparedAttempt:
    """The immutable representation, protocol parameters and allowance sent on one route."""

    route: ModelRoute
    compiled: CompiledPrompt
    parameters: dict[str, JsonValue]
    max_output_tokens: int
    estimated_text_tokens: int
    attempt_number: int
    sdk_messages: tuple[ModelMessage, ...]

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            **self.compiled.to_dict(),
            "model_id": self.route.model_id,
            "requested_model_name": self.route.model.model_name,
            "parameters": self.parameters,
            "max_output_tokens": self.max_output_tokens,
            "estimated_text_tokens": self.estimated_text_tokens,
            "media_tokens": None,
            "input_token_count_kind": "utf8-bytes/3 estimate; media unknown",
            "attempt_number": self.attempt_number,
        }


@dataclass(frozen=True)
class PreparedGeneration:
    """Keep the selected input compiled and later routes deferred across this stage."""

    task: str
    assignment: TaskAssignment
    compiled: CompiledPrompt
    routes: tuple[ModelRoute, ...]
    snapshot: ConfigSnapshot
    max_attempts: int
    timeout_seconds: float
    max_output_tokens: int
    structured_retries: int
    estimated_input_tokens: int
    stage_refs: tuple[ResultRef, ...] = ()
    ineligible_routes: dict[str, JsonValue] = field(default_factory=dict)
    attempt_plans: tuple[PreparedAttempt, ...] = ()
    coverage: dict[str, JsonValue] = field(default_factory=dict)

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            **self.compiled.to_dict(),
            "task": self.task,
            "model_id": self.routes[0].model_id,
            "requested_model_name": self.routes[0].model.model_name,
            "output_mode": self.assignment.output_mode,
            "parameters": self.attempt_plans[0].parameters
            if self.attempt_plans
            else self.routes[0].preset.model_dump(mode="json"),
            "max_output_tokens": self.max_output_tokens,
            "estimated_input_tokens": self.estimated_input_tokens,
            "input_token_count_kind": "estimated",
            "config_version": self.snapshot.version,
            "ineligible_routes": self.ineligible_routes,
            "deferred_routes": [route.model_id for route in self.routes[1:]],
            "media_tokens": None,
            "prepared_attempts": [attempt.to_dict() for attempt in self.attempt_plans],
            "stage_result_refs": [item.model_dump(mode="json") for item in self.stage_refs],
        }


def estimate_text(compiled: CompiledPrompt) -> int:
    length = sum(
        len(str(part.get("text", "")).encode())
        for message in compiled.preview
        for part in cast(list[dict[str, JsonValue]], message["content"])
        if part.get("type") == "text"
    )
    return math.ceil(
        (length + (len(json.dumps(compiled.schema).encode()) if compiled.schema else 0)) / 3
    )
