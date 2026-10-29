"""Provider-neutral typed generation and all three media interpretation paths."""

import asyncio
import hashlib
import json
from contextlib import AsyncExitStack
from dataclasses import dataclass
from typing import TypeVar

from nous_wave.contracts import CallContext, DomainError
from pydantic import BaseModel, JsonValue, TypeAdapter
from pydantic_ai import Agent, BinaryContent, NativeOutput, capture_run_messages
from pydantic_ai.capabilities import Instrumentation
from pydantic_ai.messages import ModelMessagesTypeAdapter
from pydantic_ai.models.instrumented import InstrumentationSettings
from pydantic_ai.models.openai import OpenAIChatModel, OpenAIChatModelSettings
from pydantic_ai.providers.openai import OpenAIProvider

from nous.platform.configuration import ModelRoute
from nous.platform.errors import NousError, Unavailable
from nous.platform.network.service import NetworkService

from .completion import require_complete_text
from .content_parts import complete_video
from .ledger import CallLedger

T = TypeVar("T", bound=BaseModel | str)


@dataclass(frozen=True)
class Generated[T: BaseModel | str]:
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
    prompt: dict[str, object],
    output_type: type[BaseModel] | type[str],
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
        "prompt": prompt,
        "schema": TypeAdapter(output_type).json_schema(),
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
        prompt: dict[str, object],
        output_type: type[T],
        media: tuple[MediaInput, ...] = (),
        source_ids: tuple[str, ...] = (),
        budget_role: str | None = None,
        invocation_id: str | None = None,
    ) -> tuple[T, str]:
        video = any(part.media_type.startswith("video/") for part in media)
        snapshot = prepare_invocation(purpose, route, prompt, output_type, media, source_ids)
        call_id, cached = await self.ledger.reserve(
            ctx,
            purpose,
            snapshot,
            route.max_calls_per_day,
            budget_role=budget_role,
            invocation_id=invocation_id,
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
                await self.ledger.finish(ctx, call_id, None, {"stage": "network_acquisition"}, code)
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
            provider = OpenAIProvider(base_url=route.base_url, api_key=key, http_client=client)
            agent = Agent(
                OpenAIChatModel(route.model, provider=provider),
                output_type=str if output_type is str else NativeOutput(output_type),
                retries=0,
                capabilities=[
                    Instrumentation(
                        settings=InstrumentationSettings(
                            include_content=False,
                            include_binary_content=False,
                            include_model_request_parameters=False,
                        )
                    )
                ],
            )
            content: list[str | BinaryContent] = [str(prompt["rendered"])]
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
                    if route.extra_body:
                        settings["extra_body"] = route.extra_body
                    result = await agent.run(content, model_settings=settings, retries=0)
                    usage = result.usage
                    usage_record.update(
                        {
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
