"""Provider-neutral typed generation and all three media interpretation paths."""

import asyncio
import hashlib
import json
from dataclasses import dataclass
from typing import Literal, TypeVar

import httpx2
from nous_wave.contracts import CallContext, FormationProposal
from pydantic import BaseModel
from pydantic_ai import Agent, BinaryContent, NativeOutput, capture_run_messages
from pydantic_ai.messages import ModelMessagesTypeAdapter
from pydantic_ai.models.openai import OpenAIChatModel
from pydantic_ai.providers.openai import OpenAIProvider

from nous.platform.configuration import ModelRoute
from nous.platform.errors import Unavailable

from .ledger import CallLedger

T = TypeVar("T", bound=BaseModel)


@dataclass(frozen=True)
class MediaInput:
    data: bytes
    media_type: str
    source_reference: str


class Description(BaseModel):
    description: str
    producer_call_id: str | None = None


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
    ) -> tuple[T, str]:
        snapshot: dict[str, object] = {
            "model": route.model,
            "base_url": route.base_url,
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
        call_id, cached = await self.ledger.reserve(ctx, purpose, snapshot, route.max_calls_per_day)
        if cached is not None:
            return output_type.model_validate(cached), call_id
        provider = OpenAIProvider(
            base_url=route.base_url, api_key=key, http_client=self.client(route.timeout_seconds)
        )
        agent = Agent(
            OpenAIChatModel(route.model, provider=provider),
            output_type=NativeOutput(output_type),
            retries=0,
        )
        content: list[str | BinaryContent] = [str(prompt["rendered"])]
        for part in media:
            if part.media_type.startswith("text/"):
                content.append(part.data.decode("utf-8"))
            else:
                content.append(
                    BinaryContent(
                        part.data, media_type=part.media_type, identifier=part.source_reference
                    )
                )
        with capture_run_messages() as messages:
            try:
                result = await agent.run(
                    content, model_settings={"max_tokens": route.max_output_tokens}, retries=0
                )
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

    async def interpret(
        self,
        ctx: CallContext,
        route: ModelRoute,
        key: str,
        prompt: dict[str, object],
        media: tuple[MediaInput, ...],
        strategy: Literal["description_only", "direct_structured", "describe_then_structure"],
    ) -> Description | FormationProposal:
        if strategy == "description_only":
            result, call_id = await self.request(
                ctx,
                "media.description",
                route,
                key,
                {
                    **prompt,
                    "rendered": "描述原始媒体可见或可听的内容，保留不确定性。\n"
                    + str(prompt["rendered"]),
                },
                Description,
                media,
            )
            return result.model_copy(update={"producer_call_id": call_id})
        if strategy == "describe_then_structure":
            description, description_call = await self.request(
                ctx,
                "media.description",
                route,
                key,
                {
                    **prompt,
                    "rendered": "描述原始媒体可见或可听的内容，保留不确定性。\n"
                    + str(prompt["rendered"]),
                },
                Description,
                media,
            )
            prompt = {
                **prompt,
                "description_call_id": description_call,
                "rendered": str(prompt["rendered"])
                + "\n已保存的描述：\n"
                + description.description,
            }
        result, call_id = await self.request(
            ctx, "media.formation", route, key, prompt, FormationProposal, media
        )
        return result.model_copy(update={"producer_call_id": call_id})
