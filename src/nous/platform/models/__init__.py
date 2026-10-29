"""Native provider SDKs share managed clients, prompts and the paid call ledger."""

import asyncio
import os
from collections.abc import Awaitable, Callable
from typing import TypeVar

from nous_wave.contracts import (
    CallContext,
    DomainError,
    EmbeddingBatch,
    MediaEmbeddingInput,
    MediaReference,
)
from nous_wave.contracts.model_roles import ModelCachePolicy
from pydantic import BaseModel, TypeAdapter

from nous.platform.configuration import EmbeddingRoute, ModelRoute, Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.configuration.service import ConfigurationService
from nous.platform.errors import Unavailable
from nous.platform.network.service import NetworkService
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.prompts import PromptService

from .contracts import model_contract, model_preview, prepare_role
from .embedding import MediaResolver, embed_media, retained_embedding
from .embedding_evidence import reusable_embedding_in
from .embedding_profile import EmbeddingProfile
from .generation import (
    Generated,
    Generation,
    MediaInput,
)
from .ledger import CallLedger
from .roles import RoleRegistry
from .text_embedding import embed_text

os.environ.setdefault("PYDANTIC_AI_NO_BANNER", "1")


T = TypeVar("T", bound=BaseModel | str)


def credential(route: ModelRoute, role_id: str | None = None) -> str:
    if not route.model:
        raise DomainError(
            "model_unavailable",
            "Model ID has not been configured for this role",
            role=role_id,
            reason="model_missing",
        )
    try:
        value = (
            route.credential_file.read_text().strip()
            if route.credential_file
            else os.environ.get(route.credential_env, "")
        )
    except OSError as error:
        raise DomainError(
            "model_unavailable",
            "Configured model credential file is unavailable",
            role=role_id,
            reason="credential_unavailable",
        ) from error
    if not value:
        raise DomainError(
            "model_unavailable",
            "Model credential is not configured",
            role=role_id,
            reason="credential_missing",
        )
    return value


class ModelService:
    reusable_embedding_in = reusable_embedding_in
    model_contract = model_contract
    model_preview = model_preview
    prepare_role = prepare_role

    def __init__(
        self,
        authority: Authority,
        settings: Settings,
        prompts: PromptService,
        configuration: ConfigurationService,
        network: NetworkService,
    ) -> None:
        self.settings = settings
        self.prompts = prompts
        self.configuration = configuration
        self.ledger = CallLedger(authority)
        self.network = network
        self.generation = Generation(self.ledger, network)
        self.media_resolver: MediaResolver | None = None
        self.preview_media_resolver: (
            Callable[[CallContext, MediaReference], Awaitable[MediaInput]] | None
        ) = None
        self.roles = RoleRegistry()

    async def status(self, ctx: CallContext) -> dict[str, object]:
        """Report configured resources and local credential readiness without probing a provider."""
        effective = await self.configuration.effective(ctx, "models")
        resources = ModelConfiguration.model_validate(effective["value"])
        roles: dict[str, object] = {}
        for role_id, role in resources.roles.items():
            routes: list[dict[str, object]] = []
            for index, route_id in enumerate(role.routes):
                model = resources.models[resources.routes[route_id].model]
                ready = False
                failure: dict[str, object] | None = None
                try:
                    resolved, _ = resources.resolve(role_id, index)
                    await asyncio.to_thread(credential, resolved, role_id)
                except DomainError as error:
                    if error.code != "model_unavailable":
                        raise
                    failure = {"code": error.code, "message": str(error), "context": error.details}
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
                        if role.purpose == "embedding"
                        else None,
                        "video_content_format": model.video_content_format,
                        "credential_ready": ready,
                        "unavailable": failure,
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

    async def render_prompt(
        self, ctx: CallContext, role_id: str, variables: dict[str, object], prompt_id: str
    ) -> dict[str, object]:
        try:
            return await self.prompts.render(ctx, variables, prompt_id)
        except KeyError as error:
            raise DomainError(
                "model_unavailable",
                "Model prompt resource is not configured",
                role=role_id,
                prompt_id=prompt_id,
                reason="prompt_missing",
            ) from error

    async def generate(
        self,
        ctx: CallContext,
        role_id: str,
        variables: dict[str, object],
        output_type: type[T],
        *,
        source_ids: tuple[str, ...] = (),
        invocation_id: str | None = None,
        cache_policy: ModelCachePolicy = "shared",
        media: tuple[MediaInput, ...] = (),
    ) -> Generated[T]:
        if invocation_id is not None:
            retained = await self.ledger.recall(ctx, role_id, invocation_id)
            if retained is not None:
                return Generated(
                    output=TypeAdapter(output_type).validate_python(retained.output),
                    call_id=retained.call_id,
                )
        prepared = await self.prepare_role(ctx, role_id, variables, output_type=output_type)
        route, prompt = prepared.route, prepared.prompt
        result, call_id = await self.generation.request(
            ctx,
            prepared.definition.invocation_purpose or role_id,
            route,
            credential(route, role_id),
            prompt,
            output_type,
            media,
            source_ids=source_ids,
            invocation_id=invocation_id,
            cache_policy=cache_policy,
            budget_role=role_id,
        )
        return Generated(output=result, call_id=call_id)

    async def prepare_embedding(self, ctx: CallContext) -> EmbeddingProfile:
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, _ = resources.resolve("embedding", require_configured=False)
        if not isinstance(route, EmbeddingRoute):
            raise DomainError(
                "model_unavailable",
                "Embedding role must resolve to an embedding model",
                role="embedding",
                reason="capability_missing",
            )
        return EmbeddingProfile(route=route)

    async def successful_embedding_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        call_id: str,
    ) -> EmbeddingBatch:
        paid = await self.ledger.successful_in(view, ctx, call_id)
        return retained_embedding(paid.result, paid.id)

    async def embed(
        self,
        ctx: CallContext,
        texts: tuple[str, ...],
        *,
        source_ids: tuple[str, ...] = (),
        invocation_id: str | None = None,
        profile: EmbeddingProfile | None = None,
    ) -> EmbeddingBatch:
        route = (profile or await self.prepare_embedding(ctx)).route
        return await embed_text(
            ctx,
            texts,
            route,
            credential(route, "embedding"),
            self.ledger,
            self.network,
            source_ids,
            invocation_id=invocation_id,
        )

    async def embed_media(
        self,
        ctx: CallContext,
        inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
        *,
        source_ids: tuple[str, ...] = (),
        invocation_id: str | None = None,
        profile: EmbeddingProfile | None = None,
    ) -> EmbeddingBatch:
        if self.media_resolver is None:
            raise Unavailable("Media resolution is not configured")
        route = (profile or await self.prepare_embedding(ctx)).route
        return await embed_media(
            ctx,
            inputs,
            route,
            credential(route, "embedding"),
            self.media_resolver,
            self.ledger,
            self.network,
            source_ids,
            invocation_id=invocation_id,
        )
