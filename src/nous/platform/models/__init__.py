"""Native provider SDKs share managed clients, prompts and the paid call ledger."""

import asyncio
import json
import os
from collections.abc import Awaitable, Callable
from typing import Literal, TypeVar

from nous_wave.contracts import CallContext, EmbeddingBatch, FormationProposal, MediaEmbeddingInput
from nous_wave.contracts.cognition import FormationDraft
from pydantic import BaseModel

from nous.platform.configuration import EmbeddingRoute, ModelRoute, Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.configuration.service import ConfigurationService
from nous.platform.errors import Unavailable
from nous.platform.persistence import Authority
from nous.platform.prompts import PromptService
from nous.platform.time import parse_timepoint

from .embedding import MediaResolver, embed_media
from .generation import (
    Description,
    DescriptionDraft,
    Generated,
    Generation,
    MediaFormationDraft,
    MediaInput,
)
from .ledger import CallLedger
from .text_embedding import embed_text

os.environ.setdefault("PYDANTIC_AI_NO_BANNER", "1")


T = TypeVar("T", bound=BaseModel)


def credential(route: ModelRoute) -> str:
    if not route.model:
        raise Unavailable("Model ID has not been configured for this role")
    try:
        value = (
            route.credential_file.read_text().strip()
            if route.credential_file
            else os.environ.get(route.credential_env, "")
        )
    except OSError as error:
        raise Unavailable("Configured model credential file is unavailable") from error
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

    async def status(self, ctx: CallContext) -> dict[str, object]:
        """Report configured resources and local credential readiness without probing a provider."""
        effective = await self.configuration.effective(ctx, "models")
        resources = ModelConfiguration.model_validate(effective["value"])
        roles: dict[str, object] = {}
        for role_id, role in resources.roles.items():
            routes: list[dict[str, object]] = []
            for index, route_id in enumerate(role.routes):
                resolved, _ = resources.resolve(role_id, index)
                model = resources.models[resources.routes[route_id].model]
                ready: bool | None = None
                if resolved.model:
                    try:
                        await asyncio.to_thread(credential, resolved)
                    except Unavailable:
                        ready = False
                    else:
                        ready = True
                routes.append(
                    {
                        "id": route_id,
                        "model": model.name,
                        "provider": model.provider,
                        "capabilities": model.capabilities,
                        "dimensions": model.dimensions,
                        "normalization": model.normalization,
                        "max_batch_items": model.max_batch_items
                        if isinstance(resolved, EmbeddingRoute)
                        else None,
                        "video_content_format": model.video_content_format,
                        "credential_ready": ready,
                        "model_configured": bool(model.name),
                    }
                )
            roles[role_id] = {
                "purpose": role.purpose,
                "prompt_id": role.prompt_id,
                "active_route": role.routes[0],
                "routes": routes,
                "max_calls_per_day": role.max_calls_per_day,
            }
        return {"origin": effective["origin"], "roles": roles, "provider_probed": False}

    async def structured(
        self,
        ctx: CallContext,
        role_id: str,
        variables: dict[str, object],
        output_type: type[T],
        *,
        source_ids: tuple[str, ...] = (),
        invocation_id: str | None = None,
    ) -> Generated[T]:
        if invocation_id is not None:
            retained = await self.ledger.recall(ctx, role_id, invocation_id)
            if retained is not None:
                return Generated(
                    output=output_type.model_validate(retained.output), call_id=retained.call_id
                )
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, role = resources.resolve(role_id)
        if role.purpose == "embedding":
            raise ValueError("Structured generation requires a text generation role")
        prompt = await self.prompts.render(ctx, variables, role.prompt_id)
        if not prompt["variable_names"]:
            prompt["rendered"] = (
                str(prompt["rendered"]) + "\nInput:\n" + json.dumps(variables, ensure_ascii=False)
            )
        result, call_id = await self.generation.request(
            ctx,
            role_id,
            route,
            credential(route),
            prompt,
            output_type,
            source_ids=source_ids,
            invocation_id=invocation_id,
        )
        return Generated(output=result, call_id=call_id)

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
            ctx, "formation", route, credential(route), prompt, FormationDraft
        )
        proposal = FormationProposal.model_validate(result.model_dump(mode="json"))
        event = parse_timepoint(proposal.event_expression) if proposal.event_expression else None
        return proposal.model_copy(update={"producer_call_id": call_id, "event": event})

    async def interpret(
        self,
        ctx: CallContext,
        text: str,
        source: dict[str, object],
        media: tuple[MediaInput, ...],
        strategy: Literal["description_only", "direct_structured", "describe_then_structure"],
        *,
        description_adopter: Callable[[Description], Awaitable[None]] | None = None,
        cached_description: Description | None = None,
    ) -> Description | FormationProposal:
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        variables: dict[str, object] = {
            "source_text": text,
            "source": source,
            "source_context": text + "\n" + json.dumps(source, ensure_ascii=False),
        }
        description = cached_description
        if strategy != "direct_structured" and description is None:
            route, role = resources.resolve("description")
            prompt = await self.prompts.render(ctx, variables, role.prompt_id)
            draft_description, call_id = await self.generation.request(
                ctx,
                "media.description",
                route,
                credential(route),
                prompt,
                DescriptionDraft,
                media,
                budget_role="description",
            )
            description = Description(
                description=draft_description.description, producer_call_id=call_id
            )
            if description_adopter is not None:
                await description_adopter(description)
        if strategy == "description_only":
            assert description is not None
            return description
        route, role = resources.resolve("formation")
        prompt = await self.prompts.render(ctx, variables, role.prompt_id)
        if description is not None:
            prompt = {
                **prompt,
                "description_call_id": description.producer_call_id,
                "rendered": str(prompt["rendered"])
                + "\n已保存的描述：\n"
                + description.description,
            }
        draft, call_id = await self.generation.request(
            ctx,
            "media.formation",
            route,
            credential(route),
            prompt,
            MediaFormationDraft,
            media,
            budget_role="formation",
        )
        result = FormationProposal.model_validate(draft.model_dump(mode="json"))
        event = parse_timepoint(result.event_expression) if result.event_expression else None
        return result.model_copy(update={"producer_call_id": call_id, "event": event})

    async def embed(
        self, ctx: CallContext, texts: tuple[str, ...], *, source_ids: tuple[str, ...] = ()
    ) -> EmbeddingBatch:
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, _ = resources.resolve("embedding")
        if not isinstance(route, EmbeddingRoute):
            raise ValueError("Embedding role must resolve to an embedding model")
        return await embed_text(
            ctx, texts, route, credential(route), self.ledger, self.generation, source_ids
        )

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
