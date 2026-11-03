"""Provider-neutral typed generation and all three media interpretation paths."""

import asyncio
import hashlib
import json
import time
from collections import OrderedDict, deque
from dataclasses import dataclass
from typing import TypeVar, cast

from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.common import utc_now
from nous_sdk.contracts.media import MediaInfo
from nous_sdk.contracts.common import ModelProvenance
from nous_sdk.contracts.model_roles import InvocationSnapshot, ModelCachePolicy
from nous_sdk.contracts.prompt_compilation import CompiledPrompt, TokenEstimate
from openai import AsyncOpenAI
from openai.types.chat.chat_completion_content_part_param import ChatCompletionContentPartParam
from pydantic import JsonValue, TypeAdapter
from pydantic_ai import Agent, BinaryContent, capture_run_messages
from pydantic_ai.capabilities import Instrumentation
from pydantic_ai.messages import (
    ModelMessagesTypeAdapter,
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    UserPromptPart,
    VideoUrl,
)
from pydantic_ai.models.instrumented import InstrumentationSettings
from pydantic_ai.models.openai import (
    OpenAIChatModel,
    OpenAIChatModelSettings,
    OpenAIResponsesModel,
)
from pydantic_ai.providers.openai import OpenAIProvider
from pydantic_ai.usage import RunUsage, UsageLimits
from typing_extensions import TypeForm

from nous.platform.configuration import ModelRoute
from nous.platform.errors import NousError, Unavailable
from nous.platform.network.service import NetworkService
from nous.platform.persistence.authority import canonical_json

from .completion import require_complete_text
from .measurement import measure
from .provider_schema import output_contract, response_format

T = TypeVar("T")


@dataclass(frozen=True)
class ModelResult[T]:
    output: T
    producer: ModelProvenance
    usage: RunUsage


@dataclass(frozen=True)
class MediaInput:
    data: bytes
    media_type: str
    source_reference: str
    media_info: MediaInfo | None = None


class VolcengineArkChatModel(OpenAIChatModel):
    """OpenAI Chat Completions with Ark's native video content part."""

    async def _map_video_url_item(self, item: VideoUrl) -> ChatCompletionContentPartParam:
        return cast(
            ChatCompletionContentPartParam,
            {"type": "video_url", "video_url": {"url": item.url}},
        )

    async def _map_binary_content_item(
        self, item: BinaryContent
    ) -> ChatCompletionContentPartParam:
        if item.is_video:
            return cast(
                ChatCompletionContentPartParam,
                {"type": "video_url", "video_url": {"url": item.data_uri}},
            )
        return await super()._map_binary_content_item(item)


@dataclass(frozen=True)
class PreparedGeneration[T]:
    task: str
    route: ModelRoute
    prompt: CompiledPrompt
    output_type: TypeForm[T]
    agent_output_type: TypeForm[object]
    model_settings: OpenAIChatModelSettings
    parameters: dict[str, JsonValue]
    media: tuple[MediaInput, ...]
    source_ids: tuple[str, ...]
    response_format: dict[str, JsonValue] | None
    schema_digest: str
    prompt_digest: str
    input_digest: str
    request_digest: str
    cache_policy: ModelCachePolicy
    history: tuple[ModelRequest | ModelResponse, ...]
    content: tuple[str | BinaryContent, ...]
    preview: InvocationSnapshot


def media_channel(media_type: str) -> str:
    if media_type.startswith("text/") or media_type == "application/json":
        return "text"
    for channel in ("image", "audio", "video"):
        if media_type.startswith(channel + "/"):
            return channel
    return "document" if media_type == "application/pdf" else "binary"


def prepare_invocation(
    purpose: str,
    route: ModelRoute,
    prompt: CompiledPrompt,
    output_type: TypeForm[object],
    media: tuple[MediaInput, ...] = (),
    source_ids: tuple[str, ...] = (),
) -> dict[str, JsonValue]:
    """The exact metadata admitted by generation, without credentials or provider I/O."""
    if any(part.media_type.startswith("video/") for part in media) and route.protocol != "volcengine-ark":
        raise DomainError(
            "model_unavailable",
            "The selected API dialect cannot send native video",
            role=purpose,
            reason="video_dialect_unsupported",
        )
    options = TypeAdapter(dict[str, JsonValue]).validate_python(
        {
            "model": route.model,
            "protocol": route.protocol,
            "timeout_seconds": route.timeout_seconds,
            "max_output_tokens": route.max_output_tokens,
            "temperature": route.temperature,
            "reasoning_effort": route.reasoning_effort,
            "provider_options": route.extra_body,
        }
    )
    prompt = prompt.model_copy(update={"model_options": options})
    output_format = response_format(
        output_type, route.model, mode=route.structured_output_mode
    )
    measurement = measure(
        {
            "model": route.model,
            "messages": [message.model_dump(mode="json") for message in prompt.messages],
            "response_format": output_format,
        },
        None,
        context_window=route.context_window,
        output_reserve=route.max_output_tokens,
        binary_media=tuple(
            (
                part.media_type,
                part.data,
                part.media_info.model_dump(mode="json") if part.media_info else None,
            )
            for part in media
        ),
        tokenizer_name=route.tokenizer,
        media_limits=route.media_limits,
    )
    if not measurement.complete:
        raise DomainError(
            "model_media_estimate_required",
            "The selected model requires a media token estimate",
            model=route.model,
            media_items=measurement.media_items,
        )
    if measurement.input_tokens + measurement.output_reserve > route.context_window:
        raise DomainError(
            "model_context_budget",
            "Required task input and output exceed the model context",
            input_tokens=measurement.input_tokens,
            output_tokens=measurement.output_reserve,
            context_window=route.context_window,
        )
    media_tokens = sum(
        int(item["tokens"]) for item in measurement.media_items if isinstance(item["tokens"], int)
    )
    prompt = prompt.model_copy(
        update={
            "token_estimate": TokenEstimate(
                text_tokens=measurement.input_tokens - media_tokens,
                media_tokens=media_tokens,
                method=measurement.method,
                estimated=measurement.estimated,
            )
        }
    )
    snapshot: dict[str, object] = {
        "purpose": purpose,
        "source_ids": list(source_ids),
        "model": route.model,
        "base_url": route.base_url,
        "protocol": route.protocol,
        "parameters": {
            "timeout_seconds": route.timeout_seconds,
            "max_output_tokens": route.max_output_tokens,
            "temperature": route.temperature,
            "reasoning_effort": route.reasoning_effort,
            "extra_body": route.extra_body,
        },
        "prompt": prompt.model_dump(mode="json"),
        "schema": TypeAdapter(output_type).json_schema(),
        "provider_response_format": output_format,
        "output_mode": "text" if output_type is str else "json_schema",
        "media": [
            {
                "sha256": hashlib.sha256(part.data).hexdigest(),
                "byte_len": len(part.data),
                "media_type": part.media_type,
                "channel": media_channel(part.media_type),
                "source_reference": part.source_reference,
                "position": len(prompt.messages),
                "ordinal": index,
                "media_info": part.media_info.model_dump(mode="json")
                if part.media_info
                else None,
            }
            for index, part in enumerate(media)
        ],
    }
    return TypeAdapter(dict[str, JsonValue]).validate_python(snapshot)


class Generation:
    def __init__(self, network: NetworkService) -> None:
        self.network = network
        self._results: OrderedDict[str, bytes] = OrderedDict()
        self._result_bytes = 0
        self._traces: deque[bytes] = deque()
        self._trace_bytes = 0

    def prepare[T](
        self,
        task: str,
        route: ModelRoute,
        prompt: CompiledPrompt,
        output_type: TypeForm[T],
        media: tuple[MediaInput, ...],
        source_ids: tuple[str, ...],
        cache_policy: ModelCachePolicy,
    ) -> PreparedGeneration[T]:
        snapshot = prepare_invocation(task, route, prompt, output_type, media, source_ids)
        compiled = CompiledPrompt.model_validate(snapshot["prompt"])
        schema = TypeAdapter(dict[str, JsonValue]).validate_python(snapshot["schema"])
        params = TypeAdapter(dict[str, JsonValue]).validate_python(snapshot["parameters"])
        messages = tuple(message.model_dump(mode="json") for message in compiled.messages)
        media_parts = tuple(
            {
                "position": len(messages),
                "ordinal": index,
                "media_type": item.media_type,
                "byte_len": len(item.data),
                "sha256": hashlib.sha256(item.data).hexdigest(),
                "source_reference": item.source_reference,
            }
            for index, item in enumerate(media)
        )
        prompt_digest = self.digest(
            {
                "template_id": compiled.template_id,
                "template_revision": compiled.template_revision,
                "stable_prefix_fingerprint": compiled.stable_prefix_fingerprint,
                "messages": messages,
            }
        )
        input_digest = self.digest(
            {
                "messages": messages,
                "media": [
                    {key: value for key, value in item.items() if key != "source_reference"}
                    for item in media_parts
                ],
            }
        )
        schema_digest = self.digest(schema)
        request_digest = self.digest(
            {
                "task": task,
                "input_digest": input_digest,
                "schema_digest": schema_digest,
                "provider": {
                    "endpoint": route.base_url,
                    "protocol": route.protocol,
                    "model": route.model,
                },
                "parameters": params,
                "response_format": snapshot["provider_response_format"],
            }
        )
        history: list[ModelRequest | ModelResponse] = []
        for message in compiled.messages:
            if message.role == "system":
                history.append(ModelRequest(parts=[SystemPromptPart(message.content)]))
            elif message.role == "user":
                history.append(ModelRequest(parts=[UserPromptPart(message.content)]))
            elif message.role == "assistant":
                history.append(ModelResponse(parts=[TextPart(message.content)]))
            else:
                raise DomainError(
                    "prompt_tool_identity",
                    "Tool messages require their native tool call identity",
                )
        content: list[str | BinaryContent] = []
        for part in media:
            if part.media_type.startswith("text/") or part.media_type == "application/json":
                content.append(part.data.decode("utf-8"))
            else:
                content.append(
                    BinaryContent(part.data, media_type=part.media_type, identifier=part.source_reference)
                )
        model_settings: OpenAIChatModelSettings = {"max_tokens": route.max_output_tokens}
        if route.temperature is not None:
            model_settings["temperature"] = route.temperature
        if route.reasoning_effort is not None:
            model_settings["openai_reasoning_effort"] = route.reasoning_effort
        if route.extra_body:
            model_settings["extra_body"] = dict(route.extra_body)
        return PreparedGeneration(
            task=task,
            route=route,
            prompt=compiled,
            output_type=output_type,
            agent_output_type=output_contract(output_type, route.structured_output_mode),
            model_settings=model_settings,
            parameters=params,
            media=media,
            source_ids=source_ids,
            response_format=TypeAdapter(dict[str, JsonValue] | None).validate_python(
                snapshot["provider_response_format"]
            ),
            schema_digest=schema_digest,
            prompt_digest=prompt_digest,
            input_digest=input_digest,
            request_digest=request_digest,
            cache_policy=cache_policy,
            history=tuple(history),
            content=tuple(content),
            preview=InvocationSnapshot.model_validate(snapshot),
        )

    @staticmethod
    def digest(value: object) -> str:
        return hashlib.sha256(canonical_json(value).encode()).hexdigest()

    def cache_get[T](self, key: str, output_type: TypeForm[T]) -> ModelResult[T] | None:
        value = self._results.pop(key, None)
        if value is None:
            return None
        self._results[key] = value
        item = json.loads(value)
        return ModelResult(
            output=TypeAdapter(output_type).validate_python(item["output"]),
            producer=ModelProvenance.model_validate(item["producer"]),
            usage=RunUsage(),
        )

    def cache_put(self, key: str, result: ModelResult[object]) -> None:
        value = canonical_json(
            {
                "output": TypeAdapter(object).dump_python(result.output, mode="json"),
                "producer": result.producer.model_dump(mode="json"),
            }
        ).encode()
        maximum_bytes = 4 * 1024 * 1024
        if len(value) > 256 * 1024:
            return
        previous = self._results.pop(key, None)
        if previous is not None:
            self._result_bytes -= len(previous)
        while self._results and (len(self._results) >= 128 or self._result_bytes + len(value) > maximum_bytes):
            _, removed = self._results.popitem(last=False)
            self._result_bytes -= len(removed)
        self._results[key] = value
        self._result_bytes += len(value)

    def create_result[T](
        self,
        *,
        output: T,
        output_type: TypeForm[T],
        usage: RunUsage,
        task: str,
        model_id: str,
        prompt_digest: str,
        schema_digest: str,
        input_digest: str,
    ) -> ModelResult[T]:
        output = TypeAdapter(output_type).validate_python(output)
        serialized = TypeAdapter(output_type).dump_python(output, mode="json")
        producer = ModelProvenance(
            model_id=model_id,
            task=task,
            prompt_digest=prompt_digest,
            schema_digest=schema_digest,
            input_digest=input_digest,
            output_digest=self.digest(serialized),
            generated_at=utc_now(),
        )
        return ModelResult(output=output, producer=producer, usage=usage)

    def _trace(
        self,
        ctx: CallContext,
        prepared: PreparedGeneration[object],
        result: ModelResult[object],
        *,
        cache_hit: bool,
        elapsed_ms: int,
    ) -> None:
        messages = [message.model_dump(mode="json") for message in prepared.prompt.messages]
        self.record_result(
            ctx,
            task=prepared.task,
            model=prepared.route.model,
            protocol=prepared.route.protocol,
            parameters=prepared.parameters,
            schema=prepared.preview.output_schema,
            response_format=prepared.response_format,
            messages=TypeAdapter(JsonValue).validate_python(messages),
            media=TypeAdapter(JsonValue).validate_python(
                [item.model_dump(mode="json", by_alias=True) for item in prepared.preview.media]
            ),
            output=result.output,
            producer=result.producer,
            usage=result.usage,
            cache_hit=cache_hit,
            elapsed_ms=elapsed_ms,
        )

    def record_result(
        self,
        ctx: CallContext,
        *,
        task: str,
        model: str,
        protocol: str,
        parameters: dict[str, JsonValue],
        schema: object,
        response_format: object,
        messages: object,
        media: object,
        output: object,
        producer: ModelProvenance,
        usage: RunUsage,
        cache_hit: bool,
        elapsed_ms: int,
    ) -> None:
        encoder = TypeAdapter(object)

        def json_value(value: object) -> JsonValue:
            return TypeAdapter(JsonValue).validate_python(
                encoder.dump_python(value, mode="json")
            )

        parameters = json_value(parameters)
        schema = json_value(schema)
        response_format = json_value(response_format)
        messages = json_value(messages)
        media = json_value(media)
        output = json_value(output)
        trace: dict[str, JsonValue] = {
            "namespace_id": ctx.namespace_id,
            "caller_id": ctx.caller_id,
            "task": task,
            "model": model,
            "protocol": protocol,
            "parameters": parameters,
            "schema": schema,
            "response_format": response_format,
            "messages": messages,
            "media": media,
            "output": output,
            "producer": producer.model_dump(mode="json"),
            "usage": TypeAdapter(RunUsage).dump_python(usage, mode="json"),
            "cache_hit": cache_hit,
            "elapsed_ms": elapsed_ms,
        }
        encoded = canonical_json(trace).encode()
        if len(encoded) > 256 * 1024:
            trace["messages"] = (
                [
                    {"role": item.get("role"), "content": str(item.get("content", ""))[:4096]}
                    for item in messages
                    if isinstance(item, dict)
                ]
                if isinstance(messages, list)
                else str(messages)[:8192]
            )
            trace["output"] = str(output)[:8192]
            encoded = canonical_json(trace).encode()
        while self._traces and self._trace_bytes + len(encoded) > 4 * 1024 * 1024:
            removed = self._traces.popleft()
            self._trace_bytes -= len(removed)
        self._traces.append(encoded)
        self._trace_bytes += len(encoded)

    def inspect(self, ctx: CallContext, *, limit: int = 20) -> tuple[dict[str, JsonValue], ...]:
        result: list[dict[str, JsonValue]] = []
        for raw in reversed(self._traces):
            value = TypeAdapter(dict[str, JsonValue]).validate_python(json.loads(raw))
            if value.get("namespace_id") == ctx.namespace_id and value.get("caller_id") == ctx.caller_id:
                result.append(value)
                if len(result) >= limit:
                    break
        return tuple(result)

    async def execute[T](
        self,
        ctx: CallContext,
        prepared: PreparedGeneration[T],
        api_key: str,
    ) -> ModelResult[T]:
        if prepared.cache_policy == "shared":
            cached = self.cache_get(prepared.request_digest, prepared.output_type)
            if cached is not None:
                self._trace(ctx, prepared, cached, cache_hit=True, elapsed_ms=0)
                return cached
        started = time.perf_counter()
        route = prepared.route
        try:
            async with self.network.client(ctx, timeout_seconds=route.timeout_seconds) as client:
                provider = OpenAIProvider(
                    openai_client=AsyncOpenAI(
                        base_url=route.base_url,
                        api_key=api_key,
                        http_client=client,
                        max_retries=0,
                    )
                )
                model = (
                    OpenAIResponsesModel(route.model, provider=provider)
                    if route.protocol == "openai-responses"
                    else VolcengineArkChatModel(route.model, provider=provider)
                    if route.protocol == "volcengine-ark"
                    else OpenAIChatModel(route.model, provider=provider)
                )
                agent = Agent[None, T](
                    model,
                    output_type=prepared.agent_output_type,
                    deps_type=type(None),
                    retries=0,
                    capabilities=[
                        Instrumentation(
                            settings=InstrumentationSettings(
                                include_content=False,
                                include_binary_content=False,
                                include_model_request_parameters=False,
                            )
                        ),
                    ],
                )
                result = await agent.run(
                    prepared.content or None,
                    message_history=list(prepared.history),
                    model_settings=prepared.model_settings,
                    retries=0,
                    usage_limits=UsageLimits(request_limit=1),
                )
                if prepared.output_type is str:
                    require_complete_text(result.response.finish_reason)
                output, usage = result.output, result.usage
        except asyncio.CancelledError:
            raise
        except Exception as error:
            code = (
                error.code
                if isinstance(error, (DomainError, NousError))
                else str(getattr(error, "status_code", type(error).__name__))
            )
            context = error.details if isinstance(error, DomainError) else error.context if isinstance(error, NousError) else {}
            raise Unavailable(
                f"Generation provider failed ({code})",
                context={**context, "task": prepared.task, "provider_code": code},
            ) from error
        result = self.create_result(
            output=output,
            output_type=prepared.output_type,
            usage=usage,
            task=prepared.task,
            model_id=route.model,
            prompt_digest=prepared.prompt_digest,
            schema_digest=prepared.schema_digest,
            input_digest=prepared.input_digest,
        )
        elapsed_ms = round((time.perf_counter() - started) * 1000)
        if prepared.cache_policy == "shared":
            self.cache_put(prepared.request_digest, result)
        self._trace(ctx, prepared, result, cache_hit=False, elapsed_ms=elapsed_ms)
        return result
