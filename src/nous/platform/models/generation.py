"""Provider-neutral typed generation and all three media interpretation paths."""

import asyncio
import hashlib
import json
from dataclasses import dataclass
from typing import TypeVar

import httpx2
from nous_wave.contracts import CallContext, FormationDraft
from nous_wave.contracts.cognition import ProposedMemory
from pydantic import BaseModel, Field
from pydantic_ai import Agent, BinaryContent, NativeOutput, capture_run_messages
from pydantic_ai.capabilities import Instrumentation
from pydantic_ai.messages import ModelMessagesTypeAdapter
from pydantic_ai.models.instrumented import InstrumentationSettings
from pydantic_ai.models.openai import OpenAIChatModel, OpenAIChatModelSettings
from pydantic_ai.providers.openai import OpenAIProvider

from nous.platform.configuration import ModelRoute
from nous.platform.errors import Unavailable

from .content_parts import complete_video
from .ledger import CallLedger

T = TypeVar("T", bound=BaseModel)


@dataclass(frozen=True)
class Generated[T: BaseModel]:
    output: T
    call_id: str


@dataclass(frozen=True)
class MediaInput:
    data: bytes
    media_type: str
    source_reference: str


class DescriptionDraft(BaseModel):
    description: str = Field(min_length=1)


class Description(DescriptionDraft):
    producer_call_id: str


class MediaFormationDraft(FormationDraft):
    memories: tuple[ProposedMemory, ...] = Field(min_length=1)


class Generation:
    def __init__(self, ledger: CallLedger) -> None:
        self.ledger = ledger
        self.clients: dict[float, httpx2.AsyncClient] = {}

    async def close(self) -> None:
        for client in self.clients.values():
            await client.aclose()
        self.clients.clear()

    def client(self, timeout: float) -> httpx2.AsyncClient:
        if timeout not in self.clients:
            self.clients[timeout] = httpx2.AsyncClient(timeout=timeout, trust_env=True)
        return self.clients[timeout]

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
        if video and route.video_content_format is None:
            raise Unavailable("Video generation requires a configured video content format")
        snapshot: dict[str, object] = {
            "source_ids": list(source_ids),
            "model": route.model,
            "base_url": route.base_url,
            "parameters": {
                "max_output_tokens": route.max_output_tokens,
                "temperature": route.temperature,
                "reasoning_effort": route.reasoning_effort,
            },
            "prompt": prompt,
            "schema": output_type.model_json_schema(),
            "media": [
                {
                    "sha256": hashlib.sha256(part.data).hexdigest(),
                    "media_type": part.media_type,
                    "source_reference": part.source_reference,
                }
                for part in media
            ],
        }
        if video:
            snapshot["video_content_format"] = route.video_content_format
        call_id, cached = await self.ledger.reserve(
            ctx,
            purpose,
            snapshot,
            route.max_calls_per_day,
            budget_role=budget_role,
            invocation_id=invocation_id,
        )
        if cached is not None:
            return output_type.model_validate(cached), call_id
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
                self.client(route.timeout_seconds),
            )
        provider = OpenAIProvider(
            base_url=route.base_url, api_key=key, http_client=self.client(route.timeout_seconds)
        )
        agent = Agent(
            OpenAIChatModel(route.model, provider=provider),
            output_type=NativeOutput(output_type),
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
        with capture_run_messages() as messages:
            try:
                settings: OpenAIChatModelSettings = {"max_tokens": route.max_output_tokens}
                if route.temperature is not None:
                    settings["temperature"] = route.temperature
                if route.reasoning_effort is not None:
                    settings["openai_reasoning_effort"] = route.reasoning_effort
                result = await agent.run(content, model_settings=settings, retries=0)
                usage = result.usage
                await self.ledger.finish(
                    ctx,
                    call_id,
                    result.output.model_dump(mode="json"),
                    {
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "messages": json.loads(result.all_messages_json()),
                    },
                )
                return result.output, call_id
            except asyncio.CancelledError:
                await asyncio.shield(
                    self.ledger.finish(ctx, call_id, None, {}, "CancelledUnknownOutcome")
                )
                raise
            except Exception as error:
                code = str(getattr(error, "status_code", type(error).__name__))
                await self.ledger.finish(
                    ctx,
                    call_id,
                    None,
                    {"messages": json.loads(ModelMessagesTypeAdapter.dump_json(messages))},
                    code,
                )
                raise Unavailable(f"Generation provider failed ({code}); call {call_id}") from error
