"""One bounded model-stage executor over the shared protocol connection."""

from __future__ import annotations

import asyncio
import json
import math
from collections.abc import AsyncGenerator, Awaitable, Callable
from contextlib import asynccontextmanager
from dataclasses import dataclass, field, replace
from typing import TYPE_CHECKING, Literal, cast

import jsonschema
from openai import APIConnectionError, APIStatusError
from pydantic import JsonValue, TypeAdapter, ValidationError
from pydantic_ai.direct import model_request, model_request_stream
from pydantic_ai.exceptions import (
    ModelAPIError,
    UnexpectedModelBehavior,
)
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    PartDeltaEvent,
    PartStartEvent,
    RetryFeedbackPart,
    RetryInfo,
    TextPart,
    TextPartDelta,
    ToolCallPart,
    ToolReturnPart,
)
from pydantic_ai.models import ModelRequestParameters
from pydantic_ai.models.openai import OpenAIChatModel, OpenAIChatModelSettings
from pydantic_ai.output import OutputObjectDefinition
from pydantic_ai.profiles.openai import OpenAIModelProfile
from pydantic_ai.providers.openai import OpenAIProvider
from pydantic_ai.tools import ToolDefinition
from pydantic_ai.usage import RequestUsage

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.models.configuration import ModelConfig, PresetConfig, TaskAssignment

from .contracts import GenerationResult, ReadingInput
from .policy import (
    attempt_reports,
    attempt_scope,
    error_report,
    model_error,
    retry_delay,
    retryable,
    route_unavailable,
    stage_deadline,
)
from .prompts import CompiledPrompt, PromptCompiler, PromptResource
from .protocol import CompatibleChatModel

if TYPE_CHECKING:
    from nous.network import ModelConnections
    from nous.runtime.execution import OperationContext


@dataclass(frozen=True)
class ModelRoute:
    model_id: str
    model: ModelConfig
    preset: PresetConfig


class ModelRegistry:
    def resolve(
        self, task: str, snapshot: ConfigSnapshot
    ) -> tuple[TaskAssignment, tuple[ModelRoute, ...]]:
        """Resolve the entire ordered route from one operation snapshot without network I/O."""
        settings = snapshot.typed_values()
        if task not in settings.tasks:
            raise NousError("model.task_unconfigured", f"No model assignment for task: {task}")
        assignment = settings.tasks[task]
        return assignment, tuple(
            ModelRoute(
                choice.model,
                settings.models[choice.model],
                settings.presets[cast(str, choice.preset or assignment.preset)]
                if (choice.preset or assignment.preset)
                else PresetConfig(),
            )
            for choice in [assignment, *assignment.fallback]
        )


@dataclass(frozen=True)
class FrozenTask:
    task: str
    assignment: TaskAssignment
    routes: tuple[ModelRoute, ...]
    resource: PromptResource | None
    ineligible_routes: dict[str, JsonValue] = field(default_factory=dict)


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
    stage_refs: tuple[str, ...] = ()
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
            "stage_result_refs": list(self.stage_refs),
        }


def _estimate_text(compiled: CompiledPrompt) -> int:
    length = sum(
        len(str(part.get("text", "")).encode())
        for message in compiled.preview
        for part in cast(list[dict[str, JsonValue]], message["content"])
        if part.get("type") == "text"
    )
    return math.ceil(
        (length + (len(json.dumps(compiled.schema).encode()) if compiled.schema else 0)) / 3
    )


@dataclass
class GenerationStream:
    """Expose deltas immediately and fill result after complete response validation."""

    deltas: AsyncGenerator[str] = field(init=False)
    result: GenerationResult | None = None

    def __aiter__(self) -> AsyncGenerator[str]:
        return self.deltas


class ModelExecutor:
    """Own model-stage execution; the caller owns operation lifetime and result adoption."""

    def __init__(self, connections: ModelConnections, compiler: PromptCompiler) -> None:
        self.connections = connections
        self.compiler = compiler
        self.registry = ModelRegistry()

    def freeze(self, task: str, snapshot: ConfigSnapshot) -> FrozenTask:
        assignment, routes = self.registry.resolve(task, snapshot)
        resource = self.compiler.catalog.get(assignment.prompt) if assignment.prompt else None
        return FrozenTask(task, assignment, routes, resource)

    def validate_route(
        self,
        frozen: FrozenTask,
        route: ModelRoute,
        snapshot: ConfigSnapshot,
        *,
        streaming: bool = False,
    ) -> None:
        """Check declared protocol/output capability before preparing its input representation."""
        self._check_route(route, frozen.assignment, None, snapshot)
        if streaming and "stream" not in route.model.capabilities:
            raise NousError(
                "model.capability_missing", f"Model {route.model_id} does not declare streaming"
            )

    def prepare(
        self,
        task: str | FrozenTask,
        reading: ReadingInput,
        snapshot: ConfigSnapshot,
        *,
        schema: dict[str, JsonValue] | None = None,
    ) -> PreparedGeneration:
        """Compile only the selected route; alternative inputs remain deferred until switching."""
        frozen = self.freeze(task, snapshot) if isinstance(task, str) else task
        assignment, routes = frozen.assignment, frozen.routes
        if assignment.operation != "generation" or frozen.resource is None:
            raise NousError(
                "model.operation_mismatch", "This task requires the transcription endpoint"
            )
        if schema is not None:
            self._validate_schema(schema)
        compiled = self.compiler.compile(frozen.resource, reading, schema=schema)
        structured = compiled.schema is not None
        if structured != (assignment.output_mode != "text"):
            raise NousError(
                "model.output_mode_mismatch", "Task output mode does not match its prompt contract"
            )
        limits = snapshot.typed_values().model_execution
        budget = assignment.budget
        output_limit = min(
            v
            for v in [
                limits.max_output_tokens,
                budget.max_output_tokens,
                routes[0].model.max_output_tokens,
                routes[0].preset.max_output_tokens,
            ]
            if v is not None
        )
        # Explicitly estimated; provider usage is retained separately.
        estimate = _estimate_text(compiled)
        input_limit = min(
            limits.max_input_tokens, budget.max_input_tokens or limits.max_input_tokens
        )
        route = routes[0]
        self._check_route(route, assignment, compiled, snapshot)
        if estimate > input_limit or (
            route.model.context_window is not None
            and estimate + output_limit > route.model.context_window
        ):
            raise NousError(
                "model.input_limit",
                "Prepared input exceeds this route's context allowance",
                details={
                    "model_id": route.model_id,
                    "estimated_input_tokens": estimate,
                    "max_output_tokens": output_limit,
                    "context_window": route.model.context_window,
                    "limit": input_limit,
                },
            )
        prepared = PreparedGeneration(
            frozen.task,
            assignment,
            compiled,
            routes,
            snapshot,
            min(limits.max_attempts, budget.max_attempts or limits.max_attempts),
            min(
                v
                for v in [
                    limits.timeout_seconds,
                    budget.timeout_seconds,
                ]
                if v is not None
            ),
            output_limit,
            limits.structured_retries,
            estimate,
            ineligible_routes=dict(frozen.ineligible_routes),
        )
        return replace(
            prepared,
            attempt_plans=(self.prepare_attempt(prepared, routes[0], 1),),
        )

    def prepare_attempt(
        self,
        prepared: PreparedGeneration,
        route: ModelRoute,
        number: int,
        messages: tuple[ModelMessage, ...] | None = None,
    ) -> PreparedAttempt:
        if route != prepared.routes[0]:
            raise NousError(
                "model.unprepared_route", "Prepare the selected route before creating its attempt"
            )
        active = prepared
        parameters = self._request_settings(active, route)
        return PreparedAttempt(
            route,
            active.compiled,
            parameters,
            int(cast(int, parameters["max_tokens"])),
            active.estimated_input_tokens,
            number,
            messages if messages is not None else active.compiled.messages,
        )

    @staticmethod
    def _validate_schema(schema: dict[str, JsonValue]) -> None:
        def refs(value: JsonValue) -> None:
            if isinstance(value, dict):
                for keyword in ("$ref", "$dynamicRef", "$recursiveRef"):
                    reference = value.get(keyword)
                    if isinstance(reference, str) and not reference.startswith("#"):
                        raise NousError(
                            "model.remote_schema_ref", "JSON Schema references must be local"
                        )
                if "$id" in value:
                    raise NousError(
                        "model.schema_id", "Schema IDs are unsupported; use local references"
                    )
                for child in value.values():
                    refs(child)
            elif isinstance(value, list):
                for child in value:
                    refs(child)

        refs(schema)
        try:
            jsonschema.Draft202012Validator.check_schema(schema)
        except jsonschema.SchemaError as error:
            raise NousError("model.schema_invalid", error.message) from error

    def _check_route(
        self,
        route: ModelRoute,
        assignment: TaskAssignment,
        compiled: CompiledPrompt | None,
        snapshot: ConfigSnapshot,
    ) -> None:
        provider = snapshot.typed_values().providers[route.model.provider]
        if provider.protocol != "openai-chat-completions":
            raise NousError(
                "model.protocol_unsupported", f"Unsupported protocol: {provider.protocol}"
            )
        if assignment.output_mode not in route.model.output_modes:
            raise NousError(
                "model.capability_missing",
                f"Model {route.model_id} does not declare {assignment.output_mode} output",
            )
        if "text" not in route.model.capabilities:
            raise NousError(
                "model.capability_missing",
                f"Model {route.model_id} does not declare text generation",
            )
        if (
            compiled.schema if compiled else assignment.output_mode != "text"
        ) and "structured_output" not in route.model.capabilities:
            raise NousError(
                "model.capability_missing",
                f"Model {route.model_id} does not declare structured output",
            )
        for source in compiled.reading.sources if compiled else ():
            for medium in source.media:
                mime = medium.media_type
                if mime.startswith("audio/") and medium.format not in {"wav", "mp3"}:
                    raise NousError(
                        "model.protocol_format_unsupported",
                        "The configured chat protocol accepts WAV or MP3 audio",
                        details={"media_type": mime, "model_id": route.model_id},
                    )
                capability = mime.split("/")[0]
                if mime not in route.model.input_formats or (
                    capability in {"image", "audio", "video"}
                    and capability not in route.model.capabilities
                ):
                    raise NousError(
                        "model.capability_missing", f"Model {route.model_id} does not accept {mime}"
                    )
        unknown = set(route.preset.extensions) - set(OpenAIChatModelSettings.__annotations__)
        if unknown:
            raise NousError(
                "model.parameter_unsupported", f"Unknown protocol settings: {sorted(unknown)}"
            )
        reserved = {"max_tokens", "max_retries", "timeout"} & set(route.preset.extensions)
        if reserved:
            raise NousError(
                "model.parameter_conflict",
                f"Use the dedicated preset or budget fields for: {sorted(reserved)}",
            )
        body_reserved = {
            "model",
            "messages",
            "stream",
            "tools",
            "tool_choice",
            "response_format",
            "max_tokens",
            "max_completion_tokens",
        }
        bodies = [route.preset.reasoning, route.preset.extensions.get("extra_body")]
        for body in bodies:
            if isinstance(body, dict) and body_reserved & set(body):
                raise NousError(
                    "model.parameter_conflict",
                    "Protocol extensions cannot replace the prepared request",
                    details={"fields": sorted(body_reserved & set(body))},
                )
        unknown_profile = set(route.model.compatibility) - set(OpenAIModelProfile.__annotations__)
        if unknown_profile:
            raise NousError(
                "model.profile_invalid",
                f"Unknown protocol compatibility fields: {sorted(unknown_profile)}",
            )
        if (
            route.model.compatibility.get("openai_chat_streaming_requires_finish_reason", True)
            is not True
        ):
            raise NousError(
                "model.profile_invalid", "Streaming completion requires an explicit finish reason"
            )

    def _model(self, route: ModelRoute) -> OpenAIChatModel:
        profile = OpenAIModelProfile(
            supports_json_schema_output="native" in route.model.output_modes,
            supports_tools="tool" in route.model.output_modes,
            supports_audio_input="audio" in route.model.capabilities,
            supports_image_input="image" in route.model.capabilities,
            supports_document_input="application/pdf" in route.model.input_formats,
            openai_chat_supports_document_input="application/pdf" in route.model.input_formats,
            openai_chat_streaming_requires_finish_reason=True,
        )
        profile.update(cast(OpenAIModelProfile, route.model.compatibility))
        return CompatibleChatModel(
            route.model.model_name,
            provider=OpenAIProvider(openai_client=self.connections.client(route.model.provider)),
            profile=profile,
        )

    def _settings(self, prepared: PreparedGeneration, route: ModelRoute) -> OpenAIChatModelSettings:
        self._check_route(route, prepared.assignment, prepared.compiled, prepared.snapshot)
        output_limit = min(
            value
            for value in (
                prepared.snapshot.typed_values().model_execution.max_output_tokens,
                prepared.assignment.budget.max_output_tokens,
                route.model.max_output_tokens,
                route.preset.max_output_tokens,
            )
            if value is not None
        )
        if (
            route.model.context_window is not None
            and prepared.estimated_input_tokens + output_limit > route.model.context_window
        ):
            raise NousError(
                "model.input_limit",
                "Prepared input exceeds this route's context allowance",
                details={"model_id": route.model_id, "context_window": route.model.context_window},
            )
        settings = OpenAIChatModelSettings(
            max_tokens=output_limit,
            max_retries=0,
            timeout=self.connections.timeout(route.model.provider, prepared.snapshot),
        )
        if route.preset.temperature is not None:
            settings["temperature"] = route.preset.temperature
        if route.preset.top_p is not None:
            settings["top_p"] = route.preset.top_p
        if route.preset.reasoning:
            settings["extra_body"] = dict(route.preset.reasoning)
        return cast(OpenAIChatModelSettings, {**settings, **route.preset.extensions})

    @staticmethod
    def _parameters(prepared: PreparedGeneration) -> ModelRequestParameters:
        schema = prepared.compiled.schema
        if schema is None:
            return ModelRequestParameters()
        # Pydantic AI owns protocol-specific schema transformation.
        if prepared.assignment.output_mode == "native":
            return ModelRequestParameters(
                output_mode="native",
                allow_text_output=False,
                output_object=OutputObjectDefinition(schema, name="result", strict=True),
            )
        return ModelRequestParameters(
            output_mode="tool",
            allow_text_output=False,
            output_tools=[
                ToolDefinition(
                    name="result",
                    parameters_json_schema=schema,
                    strict=True,
                    kind="output",
                )
            ],
        )

    def _request_settings(
        self, prepared: PreparedGeneration, route: ModelRoute
    ) -> dict[str, JsonValue]:
        """Persist the effective SDK parameters without serializing connection resources."""
        settings = dict(self._settings(prepared, route))
        settings.pop("timeout", None)
        timeout = self.connections.timeout(route.model.provider, prepared.snapshot)
        return {
            **TypeAdapter(dict[str, JsonValue]).validate_python(settings),
            "attempt_timeout_seconds": route.preset.timeout_seconds,
            "transport_timeout_seconds": {
                "connect": timeout.connect,
                "read": timeout.read,
                "write": timeout.write,
                "pool": timeout.pool,
            },
        }

    def _result(
        self,
        prepared: PreparedGeneration,
        route: ModelRoute,
        response: ModelResponse,
        attempts: int,
    ) -> GenerationResult:
        if response.state != "complete" or response.finish_reason in {
            "length",
            "content_filter",
            "error",
        }:
            raise NousError(
                "model.incomplete_output",
                "The provider did not complete the requested output",
                details={
                    "finish_reason": response.finish_reason,
                    "response_state": response.state,
                    "raw_output": response.text,
                    "attempts": attempts,
                },
            )
        output: JsonValue = response.text or ""
        if prepared.compiled.schema is not None:
            tool = next(
                (
                    part
                    for part in response.parts
                    if isinstance(part, ToolCallPart) and part.tool_name == "result"
                ),
                None,
            )
            raw = tool.args if tool else response.text
            try:
                parsed: object = json.loads(raw) if isinstance(raw, str) else raw
                validate = cast(
                    Callable[[object], None],
                    jsonschema.Draft202012Validator(prepared.compiled.schema).validate,
                )
                validate(parsed)
                output = prepared.compiled.validate_output(
                    TypeAdapter(JsonValue).validate_python(parsed), prepared.compiled.reading
                )
            except (ValueError, ValidationError, jsonschema.ValidationError) as error:
                raise NousError(
                    "model.output_invalid",
                    "Model output failed the task contract",
                    details={"problem": str(error), "raw_output": raw, "attempts": attempts},
                ) from error
        if prepared.compiled.schema is None and isinstance(output, str) and not output.strip():
            raise NousError("model.empty_output", "The provider returned no task output")
        producer: dict[str, JsonValue] = {
            "kind": "model",
            "provider": route.model.provider,
            "protocol": "openai-chat-completions",
            "model_id": route.model_id,
            "requested_model_name": route.model.model_name,
            "returned_model_name": response.model_name,
            "deployment_revision": route.model.deployment_revision,
            "request_id": response.provider_response_id,
            "generated_at": response.timestamp.isoformat(),
            "template_version": prepared.compiled.resource.version,
        }
        usage = TypeAdapter(dict[str, JsonValue]).validate_python(response.usage.__dict__)
        return GenerationResult(
            output,
            producer,
            usage,
            {
                **prepared.to_dict(),
                "model_id": route.model_id,
                "requested_model_name": route.model.model_name,
                "parameters": self._request_settings(prepared, route),
            },
            attempts,
            response.finish_reason,
            # SDK response parts can contain datetimes; persistence receives only JSON values.
            TypeAdapter(dict[str, JsonValue]).validate_python(
                TypeAdapter(ModelResponse).dump_python(response, mode="json")
            ),
            reading=prepared.compiled.reading,
            coverage=prepared.coverage,
        )

    @staticmethod
    async def _prepare_route(
        initial: PreparedGeneration,
        index: int,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None,
        unavailable: dict[str, JsonValue],
    ) -> tuple[PreparedGeneration, int]:
        if prepare_route is None:
            raise NousError(
                "model.route_preparation_required", "The next route requires its own prepared input"
            )
        for candidate_index in range(min(index, len(initial.routes) - 1), len(initial.routes)):
            route = initial.routes[candidate_index]
            try:
                return await prepare_route(route), candidate_index
            except NousError as error:
                if not route_unavailable(error):
                    raise
                unavailable[route.model_id] = error.to_dict()
        raise NousError(
            "model.no_eligible_route",
            "No remaining route can prepare this request",
            details={"routes": unavailable},
        )

    async def execute(
        self,
        prepared: PreparedGeneration,
        context: OperationContext | None = None,
        *,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None = None,
    ) -> GenerationResult:
        """Share one attempt count across transport retry, fallback and output repair."""
        repairs = 0
        route_index = 0
        previous_route: ModelRoute | None = None
        messages: list[ModelMessage] = []
        total_usage = RequestUsage()
        unknown_usage = False
        earlier_responses: list[JsonValue] = []
        repair_feedbacks: list[JsonValue] = []
        report_start = len(attempt_reports())
        active = prepared
        unavailable = dict(prepared.ineligible_routes)
        async with asyncio.timeout_at(stage_deadline(prepared.timeout_seconds, context)):
            for attempt in range(1, prepared.max_attempts + 1):
                route = prepared.routes[min(route_index, len(prepared.routes) - 1)]
                if route != active.routes[0]:
                    active, route_index = await self._prepare_route(
                        prepared, route_index, prepare_route, unavailable
                    )
                    route = active.routes[0]
                if not messages or route != previous_route:
                    messages = list(active.compiled.messages)
                    previous_route = route
                submission = self.prepare_attempt(active, route, attempt, tuple(messages))
                response: ModelResponse | None = None
                result: GenerationResult | None = None
                retry_error: BaseException | None = None
                with attempt_scope(
                    attempt,
                    {
                        "model_id": route.model_id,
                        "provider": route.model.provider,
                        "model_name": route.model.model_name,
                        "protocol": "openai-chat-completions",
                    },
                    prepared.task,
                ) as scope:
                    try:
                        async with asyncio.timeout(route.preset.timeout_seconds):
                            response = await model_request(
                                self._model(route),
                                list(submission.sdk_messages),
                                model_settings=self._settings(active, route),
                                model_request_parameters=self._parameters(active),
                            )
                        total_usage.incr(response.usage)
                        result = self._result(active, route, response, attempt)
                        scope.complete(
                            outcome="success",
                            provider_request_id=response.provider_response_id,
                            reported_usage=result.usage,
                        )
                    except (
                        APIConnectionError,
                        APIStatusError,
                        TimeoutError,
                        ModelAPIError,
                        UnexpectedModelBehavior,
                        NousError,
                    ) as error:
                        repair = (
                            isinstance(error, NousError)
                            and error.code == "model.output_invalid"
                            and repairs < prepared.structured_retries
                        )
                        retry = attempt < prepared.max_attempts and (retryable(error) or repair)
                        report = error_report(
                            error,
                            repair=repair,
                            next_action=(
                                "repair_same_route"
                                if repair
                                else "next_route"
                                if route_index + 1 < len(prepared.routes)
                                else "retry_same_route"
                            )
                            if retry
                            else "raise",
                        )
                        if response is not None:
                            report["provider_request_id"] = response.provider_response_id
                            scope.complete(
                                **report,
                                reported_usage=TypeAdapter(dict[str, JsonValue]).validate_python(
                                    response.usage.__dict__
                                ),
                            )
                        else:
                            unknown_usage = True
                            scope.complete(**report)
                        if not retry:
                            raise model_error(error, attempt) from error
                        repairs += int(repair)
                        if not repair:
                            route_index += 1
                        if repair and response is not None and isinstance(error, NousError):
                            earlier_responses.append(
                                TypeAdapter(JsonValue).validate_python(
                                    TypeAdapter(ModelResponse).dump_python(response, mode="json")
                                )
                            )
                            messages.append(response)
                            feedback = (
                                "Return a corrected output satisfying the same task schema. "
                                + str(error.details.get("problem", "Invalid output"))
                            )
                            repair_feedbacks.append(
                                {"feedback": feedback, "after_attempt": attempt}
                            )
                            calls = [
                                part for part in response.parts if isinstance(part, ToolCallPart)
                            ]
                            retry_info: RetryInfo[Literal["validation_error"]] = RetryInfo(
                                cause="validation_error",
                                attempt=repairs,
                                max_retries=prepared.structured_retries,
                            )
                            messages.append(
                                ModelRequest(
                                    parts=[
                                        ToolReturnPart(
                                            call.tool_name,
                                            feedback,
                                            call.tool_call_id,
                                            outcome="retried",
                                            retry=retry_info,
                                        )
                                        for call in calls
                                    ]
                                    if calls
                                    else [RetryFeedbackPart(feedback, retry=retry_info)]
                                )
                            )
                        retry_error = error
                if retry_error is not None:
                    await retry_delay(retry_error)
                    continue
                assert result is not None
                return replace(
                    result,
                    usage={
                        **TypeAdapter(dict[str, JsonValue]).validate_python(total_usage.__dict__),
                        "completeness": "partial" if unknown_usage else "reported",
                    },
                    response={**result.response, "earlier_responses": earlier_responses},
                    request={
                        **result.request,
                        "prepared_attempt": submission.to_dict(),
                        "attempt_reports": attempt_reports(report_start),
                        "ineligible_routes": unavailable,
                        "structured_repairs": repairs,
                        "repair_feedbacks": repair_feedbacks,
                    },
                )
        raise AssertionError("attempt limit must be positive")

    @asynccontextmanager
    async def stream(
        self,
        prepared: PreparedGeneration,
        context: OperationContext | None = None,
        *,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None = None,
    ) -> AsyncGenerator[GenerationStream]:
        """Close the active SDK response when iteration finishes, fails or is abandoned."""
        if prepared.compiled.schema is not None:
            raise NousError(
                "model.stream_structured", "Structured output is returned after complete validation"
            )
        stream = GenerationStream()
        # The generator owns and closes every response, including early exit by the consumer.
        stream.deltas = self._deltas(prepared, context, stream, prepare_route)
        try:
            yield stream
        finally:
            await stream.deltas.aclose()

    async def _deltas(
        self,
        prepared: PreparedGeneration,
        context: OperationContext | None,
        target: GenerationStream | None = None,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None = None,
    ) -> AsyncGenerator[str]:
        visible = False
        report_start = len(attempt_reports())
        active = prepared
        route_index = 0
        unavailable = dict(prepared.ineligible_routes)
        async with asyncio.timeout_at(stage_deadline(prepared.timeout_seconds, context)):
            for attempt in range(1, prepared.max_attempts + 1):
                route = prepared.routes[min(route_index, len(prepared.routes) - 1)]
                if route != active.routes[0]:
                    active, route_index = await self._prepare_route(
                        prepared, route_index, prepare_route, unavailable
                    )
                    route = active.routes[0]
                if "stream" not in route.model.capabilities:
                    raise NousError(
                        "model.capability_missing",
                        f"Model {route.model_id} does not declare streaming",
                    )
                submission = self.prepare_attempt(active, route, attempt)
                retry_error: BaseException | None = None
                result: GenerationResult | None = None
                with attempt_scope(
                    attempt,
                    {"model_id": route.model_id, "provider": route.model.provider},
                    prepared.task,
                ) as scope:
                    try:
                        async with (
                            asyncio.timeout(route.preset.timeout_seconds),
                            model_request_stream(
                                self._model(route),
                                list(submission.sdk_messages),
                                model_settings=self._settings(active, route),
                                model_request_parameters=self._parameters(active),
                            ) as response,
                        ):
                            async for event in response:
                                delta = (
                                    event.part.content
                                    if isinstance(event, PartStartEvent)
                                    and isinstance(event.part, TextPart)
                                    else event.delta.content_delta
                                    if isinstance(event, PartDeltaEvent)
                                    and isinstance(event.delta, TextPartDelta)
                                    else ""
                                )
                                if delta:
                                    visible = True
                                    yield delta
                            result = self._result(active, route, response.get(), attempt)
                            scope.complete(
                                outcome="success",
                                provider_request_id=cast(
                                    str | None, result.producer.get("request_id")
                                ),
                                reported_usage=result.usage,
                            )
                    except asyncio.CancelledError, GeneratorExit:
                        scope.complete(outcome="cancelled", next_action="close_stream")
                        raise
                    except (
                        APIConnectionError,
                        APIStatusError,
                        TimeoutError,
                        ModelAPIError,
                        UnexpectedModelBehavior,
                        NousError,
                    ) as error:
                        retry = not visible and attempt < prepared.max_attempts and retryable(error)
                        scope.complete(
                            **error_report(
                                error,
                                next_action=(
                                    "next_route"
                                    if route_index + 1 < len(prepared.routes)
                                    else "retry_same_route"
                                )
                                if retry
                                else "raise",
                            )
                        )
                        if not retry:
                            failure = model_error(error, attempt)
                            failure.details["partial_output_delivered"] = visible
                            raise failure from error
                        route_index += 1
                        retry_error = error
                if retry_error is not None:
                    await retry_delay(retry_error)
                    continue
                assert result is not None
                if target is not None:
                    target.result = replace(
                        result,
                        request={
                            **result.request,
                            "prepared_attempt": submission.to_dict(),
                            "attempt_reports": attempt_reports(report_start),
                            "ineligible_routes": unavailable,
                        },
                    )
                return

    async def discover(self, provider: str, snapshot: ConfigSnapshot) -> list[str]:
        try:
            page = await self.connections.client(provider).models.list(
                timeout=self.connections.timeout(provider, snapshot)
            )
            return [item.id async for item in page]
        except (APIConnectionError, APIStatusError, TimeoutError) as error:
            raise model_error(error, 1) from error

    async def transcribe(
        self,
        task: str | FrozenTask,
        audio: bytes,
        filename: str,
        snapshot: ConfigSnapshot,
        *,
        language: str | None = None,
        hint: str | None = None,
        context: OperationContext | None = None,
    ) -> GenerationResult:
        """Upload audio through the shared connection under the same stage limits as chat."""
        frozen = self.freeze(task, snapshot) if isinstance(task, str) else task
        assignment, routes = frozen.assignment, frozen.routes
        if assignment.operation != "transcription":
            raise NousError(
                "model.operation_mismatch", "Transcription requires a transcription task"
            )
        if frozen.resource is not None:
            compiled = self.compiler.compile(frozen.resource, ReadingInput(instructions=hint or ""))
            if len(compiled.preview) != 1:
                raise NousError(
                    "model.transcription_prompt", "Transcription accepts exactly one hint message"
                )
            hint = "\n".join(
                str(part.get("text", ""))
                for part in cast(list[dict[str, JsonValue]], compiled.preview[0]["content"])
            )
        limits = snapshot.typed_values().model_execution
        maximum = min(limits.max_attempts, assignment.budget.max_attempts or limits.max_attempts)
        timeout = min(
            limits.timeout_seconds, assignment.budget.timeout_seconds or limits.timeout_seconds
        )
        report_start = len(attempt_reports())
        async with asyncio.timeout_at(stage_deadline(timeout, context)):
            for attempt in range(1, maximum + 1):
                route = routes[min(attempt - 1, len(routes) - 1)]
                retry_error: BaseException | None = None
                generated: GenerationResult | None = None
                with attempt_scope(
                    attempt,
                    {
                        "model_id": route.model_id,
                        "provider": route.model.provider,
                        "model_name": route.model.model_name,
                        "endpoint": "audio.transcriptions",
                    },
                    frozen.task,
                ) as scope:
                    try:
                        from openai import omit

                        async with asyncio.timeout(route.preset.timeout_seconds):
                            response = await self.connections.client(
                                route.model.provider
                            ).audio.transcriptions.create(
                                file=(filename, audio),
                                model=route.model.model_name,
                                language=language if language else omit,
                                prompt=hint if hint else omit,
                                timeout=self.connections.timeout(route.model.provider, snapshot),
                            )
                        raw = response.model_dump(mode="json")
                        usage = TypeAdapter(dict[str, JsonValue]).validate_python(
                            raw.get("usage") or {}
                        )
                        generated = GenerationResult(
                            response.text,
                            {
                                "kind": "model",
                                "provider": route.model.provider,
                                "protocol": "openai-chat-completions",
                                "model_id": route.model_id,
                                "requested_model_name": route.model.model_name,
                                "returned_model_name": None,
                                "deployment_revision": route.model.deployment_revision,
                            },
                            usage,
                            {
                                "task": frozen.task,
                                "filename": filename,
                                "bytes": len(audio),
                                "language": language,
                                "hint": hint,
                            },
                            attempt,
                            response=TypeAdapter(dict[str, JsonValue]).validate_python(raw),
                        )
                        scope.complete(outcome="success", reported_usage=usage)
                    except (APIConnectionError, APIStatusError, TimeoutError) as error:
                        retry = attempt < maximum and retryable(error)
                        scope.complete(
                            **error_report(
                                error,
                                next_action=(
                                    "next_route" if attempt < len(routes) else "retry_same_route"
                                )
                                if retry
                                else "raise",
                            )
                        )
                        if not retry:
                            raise model_error(error, attempt) from error
                        retry_error = error
                if retry_error is not None:
                    await retry_delay(retry_error)
                    continue
                assert generated is not None
                return replace(
                    generated,
                    request={**generated.request, "attempt_reports": attempt_reports(report_start)},
                )
        raise AssertionError("attempt limit must be positive")
