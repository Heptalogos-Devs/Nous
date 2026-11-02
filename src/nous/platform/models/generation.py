"""Provider-neutral typed generation and all three media interpretation paths."""

import asyncio
import hashlib
import json
from contextlib import AsyncExitStack
from dataclasses import dataclass
from typing import TypeVar

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.model_roles import ModelCachePolicy
from nous_sdk.contracts.model_routing import ModelRouteAttempt
from nous_sdk.contracts.prompt_compilation import CompiledPrompt, TokenEstimate
from openai import AsyncOpenAI
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
)
from pydantic_ai.models.instrumented import InstrumentationSettings
from pydantic_ai.models.openai import OpenAIChatModel, OpenAIChatModelSettings, OpenAIResponsesModel
from pydantic_ai.providers.openai import OpenAIProvider
from pydantic_ai.usage import UsageLimits
from typing_extensions import TypeForm

from nous.platform.configuration import ModelRoute
from nous.platform.errors import NousError, Unavailable
from nous.platform.network.service import NetworkService

from .completion import require_complete_text
from .content_parts import complete_video
from .ledger import CallLedger
from .measurement import measure
from .outcomes import outcome
from .provider_schema import output_contract, response_format

T = TypeVar("T")


@dataclass(frozen=True)
class Generated[T]:
    output: T
    call_id: str


@dataclass(frozen=True)
class MediaInput:
    data: bytes
    media_type: str
    source_reference: str


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
    if (
        any(part.media_type.startswith("video/") for part in media)
        and route.video_content_format is None
    ):
        raise DomainError(
            "model_unavailable",
            "Video generation requires a configured video content format",
            role=purpose,
            reason="video_protocol_missing",
        )
    options = TypeAdapter(dict[str, JsonValue]).validate_python(
        {
            "model": route.model,
            "max_output_tokens": route.max_output_tokens,
            "temperature": route.temperature,
            "reasoning_effort": route.reasoning_effort,
            "provider_options": route.extra_body,
        }
    )
    prompt = prompt.model_copy(update={"model_options": options})
    backend = (
        "openai_sdk"
        if any(part.media_type.startswith("video/") for part in media)
        else "pydantic_ai"
    )
    output_format = response_format(
        output_type, route.model, backend=backend, mode=route.structured_output_mode
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
        binary_media=tuple((part.media_type, part.data) for part in media),
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
        "parameters": {
            "max_output_tokens": route.max_output_tokens,
            "temperature": route.temperature,
            "reasoning_effort": route.reasoning_effort,
            "extra_body": route.extra_body,
        },
        "prompt": prompt.model_dump(mode="json"),
        "schema": TypeAdapter(output_type).json_schema(),
        "provider_schema_backend": backend,
        "provider_response_format": output_format,
        "output_mode": "text" if output_type is str else "json_schema",
        "media": [
            {
                "sha256": hashlib.sha256(part.data).hexdigest(),
                "byte_len": len(part.data),
                "media_type": part.media_type,
                "channel": media_channel(part.media_type),
                "source_reference": part.source_reference,
            }
            for part in media
        ],
    }
    if any(part.media_type.startswith("video/") for part in media):
        snapshot["video_content_format"] = route.video_content_format
    return TypeAdapter(dict[str, JsonValue]).validate_python(snapshot)


class Generation:
    def __init__(self, ledger: CallLedger, network: NetworkService) -> None:
        self.ledger = ledger
        self.network = network

    async def request(
        self,
        ctx: CallContext,
        purpose: str,
        route: ModelRoute,
        key: str,
        prompt: CompiledPrompt,
        output_type: TypeForm[T],
        media: tuple[MediaInput, ...] = (),
        source_ids: tuple[str, ...] = (),
        budget_role: str | None = None,
        invocation_id: str | None = None,
        cache_policy: ModelCachePolicy = "shared",
        routing: ModelRouteAttempt | None = None,
        request_snapshot: dict[str, JsonValue] | None = None,
    ) -> tuple[T, str]:
        video = any(part.media_type.startswith("video/") for part in media)
        snapshot = prepare_invocation(purpose, route, prompt, output_type, media, source_ids)
        if request_snapshot is not None:
            snapshot = dict(request_snapshot)
        if cache_policy != "shared":
            snapshot["cache_policy"] = cache_policy
        call_id, cached = await self.ledger.reserve(
            ctx,
            purpose,
            snapshot,
            route.max_calls_per_day,
            budget_resource=route.budget_resource,
            resource_maximum=route.resource_request_budget,
            budget_role=budget_role,
            invocation_id=invocation_id,
            cache_policy=cache_policy,
            routing=routing,
        )
        if cached is not None:
            return TypeAdapter(output_type).validate_python(cached), call_id
        async with AsyncExitStack() as resources:
            try:
                client = await resources.enter_async_context(
                    self.network.client(ctx, timeout_seconds=route.timeout_seconds)
                )
            except asyncio.CancelledError:
                await asyncio.shield(
                    self.ledger.finish(
                        ctx,
                        call_id,
                        None,
                        {"stage": "network_acquisition"},
                        "CancelledBeforeRequest",
                    )
                )
                raise
            except Exception as error:
                code = error.code if isinstance(error, NousError) else type(error).__name__
                await self.ledger.finish(
                    ctx,
                    call_id,
                    None,
                    {"stage": "network_acquisition"},
                    code,
                    outcome=outcome(error),
                )
                raise Unavailable(
                    "Model network client is unavailable",
                    context={
                        **(error.context if isinstance(error, NousError) else {}),
                        "call_id": call_id,
                        "invocation_id": invocation_id,
                        "role": budget_role or purpose,
                        "provider_code": code,
                    },
                ) from error
            if video:
                return await complete_video(
                    ctx,
                    call_id,
                    route,
                    key,
                    prompt,
                    output_type,
                    media,
                    self.ledger,
                    client,
                    role=budget_role or purpose,
                    invocation_id=invocation_id,
                )
            provider = OpenAIProvider(
                openai_client=AsyncOpenAI(
                    base_url=route.base_url,
                    api_key=key,
                    http_client=client,
                    max_retries=0,
                )
            )
            agent = Agent[None, T](
                OpenAIResponsesModel(route.model, provider=provider)
                if route.protocol == "openai-responses"
                else OpenAIChatModel(route.model, provider=provider),
                output_type=output_contract(output_type, route.structured_output_mode),
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
            history: list[ModelRequest | ModelResponse] = []
            for message in prompt.messages:
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
                        BinaryContent(
                            part.data, media_type=part.media_type, identifier=part.source_reference
                        )
                    )
            usage_record: dict[str, object] = {
                "actual_output_mode": "text" if output_type is str else "json_schema"
            }
            with capture_run_messages() as messages:
                try:
                    settings: OpenAIChatModelSettings = {"max_tokens": route.max_output_tokens}
                    if route.temperature is not None:
                        settings["temperature"] = route.temperature
                    if route.reasoning_effort is not None:
                        settings["openai_reasoning_effort"] = route.reasoning_effort
                    extra_body: dict[str, JsonValue] = dict(route.extra_body)
                    if extra_body:
                        settings["extra_body"] = extra_body
                    result = await agent.run(
                        content or None,
                        message_history=history,
                        model_settings=settings,
                        retries=0,
                        usage_limits=UsageLimits(request_limit=1),
                    )
                    usage = result.usage
                    usage_record.update(
                        {
                            "requests": usage.requests,
                            "cached_input_tokens": usage.cache_read_tokens,
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                            "finish_reason": result.response.finish_reason,
                            "messages": json.loads(result.all_messages_json()),
                        }
                    )
                    if output_type is str:
                        require_complete_text(result.response.finish_reason)
                    await self.ledger.finish(
                        ctx,
                        call_id,
                        TypeAdapter(output_type).dump_python(result.output, mode="json"),
                        usage_record,
                    )
                    return TypeAdapter(output_type).validate_python(result.output), call_id
                except asyncio.CancelledError:
                    await asyncio.shield(
                        self.ledger.finish(ctx, call_id, None, {}, "CancelledUnknownOutcome")
                    )
                    raise
                except Exception as error:
                    responses = [item for item in messages if isinstance(item, ModelResponse)]
                    if responses:
                        usage_record.update(
                            {
                                "input_tokens": sum(item.usage.input_tokens for item in responses),
                                "output_tokens": sum(
                                    item.usage.output_tokens for item in responses
                                ),
                                "finish_reason": responses[-1].finish_reason,
                            }
                        )
                    code = (
                        error.code
                        if isinstance(error, (DomainError, NousError))
                        else str(getattr(error, "status_code", type(error).__name__))
                    )
                    await self.ledger.finish(
                        ctx,
                        call_id,
                        None,
                        {
                            **usage_record,
                            "messages": json.loads(ModelMessagesTypeAdapter.dump_json(messages)),
                        },
                        code,
                        outcome=outcome(
                            error,
                            response_received=any(
                                isinstance(item, ModelResponse) for item in messages
                            ),
                        ),
                    )
                    raise Unavailable(
                        f"Generation provider failed ({code})",
                        context={
                            **(
                                error.details
                                if isinstance(error, DomainError)
                                else error.context
                                if isinstance(error, NousError)
                                else {}
                            ),
                            "call_id": call_id,
                            "invocation_id": invocation_id,
                            "role": budget_role or purpose,
                            "provider_code": code,
                        },
                    ) from error
