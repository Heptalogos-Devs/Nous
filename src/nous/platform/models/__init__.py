"""Native provider SDKs share managed clients, prompts and the paid call ledger."""

import os
from typing import Literal

from nous_wave.contracts import CallContext, EmbeddingBatch, FormationProposal, MediaEmbeddingInput
from openai import AsyncOpenAI

from nous.platform.configuration import EmbeddingRoute, ModelRoute, Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.configuration.service import ConfigurationService
from nous.platform.errors import Unavailable
from nous.platform.persistence import Authority
from nous.platform.prompts import PromptService
from nous.platform.time import parse_timepoint

from .embedding import MediaResolver, embed_media, vector_space
from .generation import Description, Generation, MediaInput
from .ledger import CallLedger

os.environ.setdefault("PYDANTIC_AI_NO_BANNER", "1")


def credential(route: ModelRoute) -> str:
    value = (
        route.credential_file.read_text().strip()
        if route.credential_file
        else os.environ.get(route.credential_env, "")
    )
    if not value:
        raise Unavailable(f"Model credential missing: {route.credential_env}")
    return value


class ModelService:
    def __init__(
        self,
        authority: Authority,
        settings: Settings,
        prompts: PromptService,
        configuration: ConfigurationService,
    ) -> None:
        self.settings = settings
        self.prompts = prompts
        self.configuration = configuration
        self.ledger = CallLedger(authority)
        self.generation = Generation(self.ledger)
        self.media_resolver: MediaResolver | None = None

    async def close(self) -> None:
        await self.generation.close()

    async def form(
        self, ctx: CallContext, text: str, source: dict[str, object]
    ) -> FormationProposal:
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, role = resources.resolve("formation")
        prompt = await self.prompts.render(
            ctx, {"source_text": text, "source": source}, role.prompt_id
        )
        result, call_id = await self.generation.request(
            ctx, "formation", route, credential(route), prompt, FormationProposal
        )
        event = (
            parse_timepoint(result.event_expression) if result.event_expression else result.event
        )
        return result.model_copy(update={"producer_call_id": call_id, "event": event})

    async def interpret(
        self,
        ctx: CallContext,
        text: str,
        source: dict[str, object],
        media: tuple[MediaInput, ...],
        strategy: Literal["description_only", "direct_structured", "describe_then_structure"],
    ) -> Description | FormationProposal:
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, role = resources.resolve("formation")
        prompt = await self.prompts.render(
            ctx, {"source_text": text, "source": source}, role.prompt_id
        )
        result = await self.generation.interpret(
            ctx, route, credential(route), prompt, media, strategy
        )
        if isinstance(result, FormationProposal) and result.event_expression:
            return result.model_copy(update={"event": parse_timepoint(result.event_expression)})
        return result

    async def embed(
        self, ctx: CallContext, texts: tuple[str, ...], *, source_ids: tuple[str, ...] = ()
    ) -> EmbeddingBatch:
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, _ = resources.resolve("embedding")
        if not isinstance(route, EmbeddingRoute):
            raise ValueError("Embedding role must resolve to an embedding model")
        key = credential(route)
        request: dict[str, object] = {
            "model": route.model,
            "base_url": route.base_url,
            "texts": list(texts),
            "source_ids": list(source_ids),
            "dimensions": route.dimensions,
            "normalization": route.normalization,
        }
        call_id, cached = await self.ledger.reserve(
            ctx, "embedding", request, route.max_calls_per_day
        )
        if cached is not None:
            return EmbeddingBatch.model_validate(cached).model_copy(update={"call_id": call_id})
        try:
            client = self.generation.client(route.timeout_seconds)
            sdk = AsyncOpenAI(
                base_url=route.base_url, api_key=key, http_client=client, max_retries=0
            )
            response = await sdk.embeddings.create(
                model=route.model, input=list(texts), dimensions=route.dimensions
            )
            vectors = [
                tuple(item.embedding) for item in sorted(response.data, key=lambda v: v.index)
            ]
            if len(vectors) != len(texts) or any(len(v) != route.dimensions for v in vectors):
                raise ValueError("Provider returned inconsistent embedding dimensions")
            if route.normalization == "l2":
                import numpy as np

                array = np.asarray(vectors, dtype=np.float64)
                norms = np.linalg.norm(array, axis=1, keepdims=True)
                vectors = [tuple(row) for row in array / np.where(norms == 0, 1, norms)]
            space = vector_space(route, response.model)
            result = EmbeddingBatch(
                model=response.model,
                space=space,
                dimensions=route.dimensions,
                vectors=tuple(vectors),
                call_id=call_id,
                call_ids=(call_id,),
            )
            await self.ledger.finish(ctx, call_id, result.model_dump(mode="json"), {})
            return result
        except Exception as error:
            code = str(getattr(error, "status_code", type(error).__name__))
            await self.ledger.finish(ctx, call_id, None, {}, code)
            raise Unavailable(f"Embedding provider failed ({code}); call {call_id}") from error

    async def embed_media(
        self,
        ctx: CallContext,
        inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
        *,
        source_ids: tuple[str, ...] = (),
    ) -> EmbeddingBatch:
        if self.media_resolver is None:
            raise Unavailable("Media resolution is not configured")
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, _ = resources.resolve("embedding")
        if not isinstance(route, EmbeddingRoute):
            raise ValueError("Embedding role must resolve to an embedding model")
        return await embed_media(
            ctx,
            inputs,
            route,
            credential(route),
            self.media_resolver,
            self.ledger,
            self.generation,
            source_ids,
        )
